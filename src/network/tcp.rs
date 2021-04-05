use log::*;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use mumble_protocol::control::{
    msgs, ClientControlCodec, ControlCodec, ControlPacket,
};
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::{Clientbound, Serverbound};
use std::convert::Into;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::time::{self, Duration};
use tokio_native_tls::{TlsConnector, TlsStream};
use tokio_util::codec::{Decoder, Framed};

use crate::VoiceStreamType;

use crate::audio::AudioCommand;
use crate::{
    config::Config, RxCommandPacket, StreamType, TxAudioDecoder,
    TxCommandPacket, TxCrypt,
};
use std::io::Error;
use std::io::ErrorKind;
use tokio::select;

type TcpSender = SplitSink<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
    ControlPacket<Serverbound>,
>;
type TcpReceiver = SplitStream<
    Framed<TlsStream<TcpStream>, ControlCodec<Serverbound, Clientbound>>,
>;

pub async fn handle(
    config: Arc<Config>,
    tx_crypt: TxCrypt,
    stream_type: StreamType,
    mut rx_command: RxCommandPacket,
    tx_command: TxCommandPacket,
    tx_audio: TxAudioDecoder,
) -> crate::Result<()> {
    let (mut sink, mut stream) = connect(&config).await?;

    // this don't wait authentication sucess, just send the request
    authenticate(&config, &mut sink).await?;

    let mut sender = tokio::spawn( async move {
        sender(&mut rx_command, &mut sink).await
    });
    let mut receiver = tokio::spawn( async move {
        receiver(tx_crypt, &stream_type, tx_audio.clone(), &mut stream).await
    });
    let mut ping = tokio::spawn( async move {
        ping(tx_command.clone()).await
    });

    select!(
        ret = &mut sender => ret?,
        ret = &mut receiver => ret?,
        ret = &mut ping => ret?,
    )
}

pub async fn sender(
    rx_tcp_sender: &mut RxCommandPacket,
    sink: &mut TcpSender,
) -> crate::Result<()> {
    loop {
        let packet = rx_tcp_sender.recv().await.ok_or_else(|| {
            Error::new(
                ErrorKind::Interrupted,
                "TCP sender unable to receive from channel",
            )
        })?;
        sink.send(packet).await?;
    }
}

pub async fn receiver(
    tx_crypt: TxCrypt,
    stream_type: &StreamType,
    tx_audio_decoder: TxAudioDecoder,
    stream: &mut TcpReceiver,
) -> crate::Result<()> {
    let mut crypt = None;
    loop {
        let packet = stream.next().await.ok_or_else(|| {
            Error::new(
                ErrorKind::Interrupted,
                "TCP receiver unable to receive from socket",
            )
        })?;
        match packet.unwrap() {
            ControlPacket::TextMessage(msg) => {
                debug!("TextMessage");
                info!(
                    "Got message from user with session ID {}: {}",
                    msg.get_actor(),
                    msg.get_message()
                );
            }
            ControlPacket::CryptSetup(msg) => {
                debug!("Crypt setup");
                //TODO check if values are filled
                //Either side may request a resync by sending the message
                //without any values filled.
                if crypt.is_none() {
                    crypt = Some(msg.into());
                } else {
                    return Err(Box::new(Error::new(
                        ErrorKind::InvalidData,
                        "A Second key received",
                    )));
                }
            }
            ControlPacket::ServerSync(_msg) => {
                debug!("Logged in");
                if crypt.is_some() {
                    tx_crypt.send(crypt.clone())?;
                    let mut stream_type = stream_type.lock().or_else(|_| {
                        Err(Box::new(Error::new(
                            ErrorKind::Interrupted,
                            "TCP receiver unable to receive from socket",
                        )))
                    })?;
                    *stream_type = Some(VoiceStreamType::TCP);
                } else {
                    return Err(Box::new(Error::new(
                        ErrorKind::InvalidData,
                        "Logged in before the key",
                    )));
                }
            }
            ControlPacket::Reject(_msg) => {
                debug!("Login rejected");
                return Err(Box::new(Error::new(
                    ErrorKind::PermissionDenied,
                    "Login rejected",
                )));
            }
            ControlPacket::Version(msg) => {
                debug!("Server Version: {:?}", msg);
            }
            ControlPacket::CodecVersion(msg) => {
                debug!("Server Codec Version: {:?}", msg);
            }
            ControlPacket::PermissionQuery(msg) => {
                debug!("Permission Query: {:?}", msg);
            }
            ControlPacket::ServerConfig(msg) => {
                debug!("ServerConfig: {:?}", msg);
            }
            ControlPacket::UserState(msg) => {
                debug!("UserState");
                if msg.has_session() {
                    let session = msg.get_session();
                    tx_audio_decoder.send(AudioCommand::AddClient(session))?;
                } else {
                    return Err(Box::new(Error::new(
                        ErrorKind::InvalidData,
                        "UserState Undefined session",
                    )));
                }
            }
            ControlPacket::UserRemove(msg) => {
                debug!("UserRemove");
                if msg.has_session() {
                    let session = msg.get_session();
                    tx_audio_decoder.send(AudioCommand::DelClient(session))?;
                } else {
                    return Err(Box::new(Error::new(
                        ErrorKind::InvalidData,
                        "UserRemove Undefined session",
                    )));
                }
            }
            ControlPacket::ChannelState(msg) => {
                debug!("ChannelState: {:?}", msg);
            }
            ControlPacket::ChannelRemove(msg) => {
                debug!("ChannelRemove: {:?}", msg);
            }
            ControlPacket::UDPTunnel(msg) => {
                debug!("received TCP audio voice");
                match *msg {
                    VoicePacket::Ping { .. } => {}
                    VoicePacket::Audio {
                        session_id,
                        // seq_num,
                        payload,
                        // position_info,
                        ..
                    } => {
                        tx_audio_decoder.send(AudioCommand::Voice(
                            VoiceStreamType::TCP,
                            session_id,
                            payload,
                        ))?;
                    }
                }
            }
            ControlPacket::Ping(msg) => {
                //TODO Pong
                debug!("Ping: {:?}", msg);
            }
            packet => {
                warn!("Received unhandled ControlPacket {:#?}", packet);
            }
        }
    }
}

pub async fn ping(tx_tcp_sender: TxCommandPacket) -> crate::Result<()> {
    let mut interval = time::interval(Duration::from_secs(1));
    let mut timestamp = 0;
    loop {
        //TODO verify response
        interval.tick().await;
        debug!("Sending TCP ping");
        let mut msg = msgs::Ping::new();
        msg.set_timestamp(timestamp);
        timestamp = timestamp.wrapping_add(1);
        tx_tcp_sender.send(msg.into())?;
    }
}

pub async fn connect(
    config: &Config,
) -> crate::Result<(TcpSender, TcpReceiver)> {
    let stream = TcpStream::connect(config.addr).await?;
    debug!("TCP connected");

    let mut builder = native_tls::TlsConnector::builder();
    builder.danger_accept_invalid_certs(!config.check_cert);
    let connector: TlsConnector = builder.build()?.into();
    let tls_stream = connector.connect(&config.host, stream).await?;
    debug!("TLS connected");

    // Wrap the TLS stream with Mumble's client-side control-channel codec
    Ok(ClientControlCodec::new().framed(tls_stream).split())
}

pub async fn authenticate(
    config: &Config,
    sink: &mut TcpSender,
) -> crate::Result<()> {
    let mut msg = Box::new(msgs::Authenticate::new());
    msg.set_username(config.username.clone());
    if let Some(password) = config.password.clone() {
        msg.set_password(password);
    }
    msg.set_opus(true);
    //send the auth request
    sink.send(ControlPacket::Authenticate(msg)).await?;
    Ok(())
}
