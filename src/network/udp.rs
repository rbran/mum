use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use log::*;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::Serverbound;
use std::convert::TryInto;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::select;
use tokio::sync::watch;
use tokio::time::{interval, timeout, Duration};
use tokio_util::udp::UdpFramed;

use crate::VoiceStreamType;

use crate::audio::AudioCommand;
use crate::config::Config;
use crate::{
    RxCrypt, RxSenderPacket, StreamType, TxAudioDecoder, TxSenderPacket,
};
use std::io::Error;
use std::io::ErrorKind;

//pub type PingRequest = (u64, SocketAddr, Box<dyn FnOnce(PongPacket)>);
type PingRequest = u64;
type TxPingRequest = watch::Sender<PingRequest>;
type RxPingRequest = watch::Receiver<PingRequest>;

type UdpSender = SplitSink<
    UdpFramed<ClientCryptState>,
    (VoicePacket<Serverbound>, SocketAddr),
>;
type UdpReceiver = SplitStream<UdpFramed<ClientCryptState>>;

pub async fn handle(
    config: Arc<Config>,
    mut rx_crypt: RxCrypt,
    stream_type: StreamType,
    mut rx_sender_packet: RxSenderPacket,
    tx_sender_packet: TxSenderPacket,
    tx_audio_decoder: TxAudioDecoder,
) -> crate::Result<()> {
    //wait for the encryption before start the loop
    rx_crypt.changed().await?;
    loop {
        let socket = connect(&config).await?;
        let (sink, stream) = {
            let crypt = rx_crypt
                .borrow()
                .clone()
                .ok_or_else(|| {
                    Box::new(Error::new(
                        ErrorKind::Interrupted,
                        "TCP receiver unable to receive from socket",
                    ))
                })?
                .try_into()?;
            let framed = UdpFramed::new(socket, crypt);
            framed.split()
        };
        let (tx_last_ping, rx_last_ping) = watch::channel(0);

        select!(
            ret = sender(Arc::clone(&config), &mut rx_sender_packet, sink) => ret,
            ret = receiver(stream, tx_audio_decoder.clone(), tx_last_ping) => ret,
            ret = ping(Arc::clone(&stream_type), tx_sender_packet.clone(), rx_last_ping) => ret,
            ret = crypt(rx_crypt.clone()) => ret,
        )?;
        debug!("UDP restarting");
    }
}

pub async fn crypt(mut rx_crypt: RxCrypt) -> crate::Result<()> {
    //TODO don't open a new connection, just update the codec on
    //UdpFramed, currently the connection is reseted
    rx_crypt.changed().await.or_else(|_e| -> crate::Result<()> {
        Err(Box::new(Error::new(
            ErrorKind::Interrupted,
            "UDP Unable to receive new crypto",
        )))
    })
}

pub async fn sender(
    config: Arc<Config>,
    rx_packet: &mut RxSenderPacket,
    mut sink: UdpSender,
) -> crate::Result<()> {
    loop {
        let packet = rx_packet.recv().await.ok_or_else(|| {
            Box::new(Error::new(
                ErrorKind::Interrupted,
                "UDP sender, Unable to receive from channel",
            ))
        })?;
        sink.send((packet, config.addr)).await?;
    }
}

pub async fn receiver(
    mut stream: UdpReceiver,
    tx_audio_decoder: TxAudioDecoder,
    tx_last_ping: TxPingRequest,
) -> crate::Result<()> {
    // initiate the crypt state
    loop {
        let packet = stream.next().await.ok_or_else(|| {
            Box::new(Error::new(
                ErrorKind::Interrupted,
                "UDP sender, Unable to receive from channel",
            ))
        })?;
        let (packet, _src_addr) = match packet {
            Ok(packet) => packet,
            Err(err) => {
                warn!("got an invalid udp packet: {}", err);
                // to be expected, considering this is the internet, just ignore it
                continue;
            }
        };
        match packet {
            VoicePacket::Ping { timestamp } => {
                debug!("UDP ping packet");
                tx_last_ping.send(timestamp)?;
            }
            VoicePacket::Audio {
                session_id,
                // seq_num,
                payload,
                // position_info,
                ..
            } => {
                debug!("UDP Voice packet");
                tx_audio_decoder.send(AudioCommand::Voice(
                    VoiceStreamType::UDP,
                    session_id,
                    payload,
                ))?;
            }
        }
    }
}

pub async fn ping(
    stream_type: StreamType,
    tx_packet: TxSenderPacket,
    mut rx_last_ping: RxPingRequest,
) -> crate::Result<()> {
    let mut last_send: u64 = 0;
    let mut interval = interval(Duration::from_secs(1));

    loop {
        //execute each 1s
        interval.tick().await;
        last_send = last_send.wrapping_add(1);
        debug!("Sending UDP ping");
        tx_packet.send(VoicePacket::Ping {
            timestamp: last_send,
        })?;

        let last_ping =
            timeout(Duration::from_secs(1), rx_last_ping.changed()).await;
        let mut stream_type = stream_type.lock().or_else(|_| {
            Err(Box::new(Error::new(
                ErrorKind::Interrupted,
                "UDP ping, Unable to lock stream_type",
            )))
        })?;
        if let Err(_) = last_ping {
            //timeout
            //timeout, didn't received UDP ping, switch to TCP
            debug!("Using TCP channel for voice");
            *stream_type = Some(VoiceStreamType::TCP);
            continue;
        } else {
            //received a ping
            let recv_timestamp = *rx_last_ping.borrow();
            if recv_timestamp == last_send {
                //received the correct packet
                debug!("Using UDP channel for voice");
                *stream_type = Some(VoiceStreamType::UDP);
            } else {
                //received but with the wrong timestamp, older packet?
                debug!("Using TCP channel for voice");
                *stream_type = Some(VoiceStreamType::TCP);
            }
        }
    }
}

pub async fn connect(_: &Config) -> crate::Result<UdpSocket> {
    // Bind UDP socket
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16)).await?;
    debug!("UDP connected");
    Ok(udp_socket)
}
