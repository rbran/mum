mod audio;
mod config;
mod error;
mod network;

use crate::audio::AudioCommand;
use network::{tcp, udp};
use std::convert::{From, TryInto};
use tokio::{select, sync::mpsc, sync::watch};

use mumble_protocol::control::msgs::CryptSetup;
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::{
    control::ControlPacket, crypt::ClientCryptState, Serverbound,
};
use std::sync::{Arc, Mutex};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VoiceStreamType {
    TCP,
    UDP,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    key: Vec<u8>,
    encrypt_nonce: Vec<u8>,
    decrypt_nonce: Vec<u8>,
}

impl From<Box<CryptSetup>> for Keys {
    fn from(input: Box<CryptSetup>) -> Self {
        Keys {
            key: input.get_key().to_vec(),
            encrypt_nonce: input.get_client_nonce().to_vec(),
            decrypt_nonce: input.get_server_nonce().to_vec(),
        }
    }
}

impl TryInto<ClientCryptState> for Keys {
    type Error = Error;
    fn try_into(self) -> std::result::Result<ClientCryptState, Self::Error> {
        let err = |_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid Key format",
            )
        };
        Ok(ClientCryptState::new_from(
            self.key.try_into().map_err(err)?,
            self.encrypt_nonce.try_into().map_err(err)?,
            self.decrypt_nonce.try_into().map_err(err)?,
        ))
    }
}

pub type Crypt = Keys;
pub type TxCrypt = watch::Sender<Option<Crypt>>;
pub type RxCrypt = watch::Receiver<Option<Crypt>>;

pub type StreamType = Arc<Mutex<Option<VoiceStreamType>>>;

pub type CommandPacket = ControlPacket<Serverbound>;
pub type TxCommandPacket = mpsc::UnboundedSender<CommandPacket>;
pub type RxCommandPacket = mpsc::UnboundedReceiver<CommandPacket>;

pub type SenderPacket = VoicePacket<Serverbound>;
pub type TxSenderPacket = mpsc::UnboundedSender<SenderPacket>;
pub type RxSenderPacket = mpsc::UnboundedReceiver<SenderPacket>;

pub type AudioDecoder = AudioCommand;
pub type TxAudioDecoder = mpsc::UnboundedSender<AudioDecoder>;
pub type RxAudioDecoder = mpsc::UnboundedReceiver<AudioDecoder>;

#[tokio::main]
async fn main() {
    if std::env::args()
        .find(|s| s.as_str() == "--version")
        .is_some()
    {
        println!("mumd {}", env!("VERSION"));
        return;
    }

    error::setup_logger(std::io::stderr(), true);

    let config =
        Arc::new(config::read_cfg().expect("Unable to read config file"));

    // received crypt
    let (tx_crypt, rx_crypt) = watch::channel::<Option<Crypt>>(None);
    // current audio output channel
    let stream_type: StreamType = Arc::new(Mutex::new(None));
    //encoded audio is sent to be decoded and played
    let (tx_audio_decoder, rx_audio_decoder) =
        mpsc::unbounded_channel::<AudioDecoder>();
    //TcpPacket to send to the server
    let (tx_command_sender, rx_command_sender) =
        mpsc::unbounded_channel::<CommandPacket>();
    //send packets to the udp tunnel
    let (tx_sender_packet, rx_sender_packet) =
        mpsc::unbounded_channel::<SenderPacket>();

    select!(
        ret = tcp::handle(
                Arc::clone(&config),
                tx_crypt,
                Arc::clone(&stream_type),
                rx_command_sender,
                tx_command_sender.clone(),
                tx_audio_decoder.clone(),
            ) => panic!("TCP exit {:#?}", ret),
        ret = udp::handle(
                Arc::clone(&config),
                rx_crypt,
                Arc::clone(&stream_type),
                rx_sender_packet,
                tx_sender_packet.clone(),
                tx_audio_decoder.clone(),
            ) => panic!("UDP exit {:#?}", ret),
        ret = audio::handle(
                Arc::clone(&config),
                Arc::clone(&stream_type),
                tx_command_sender.clone(),
                tx_sender_packet.clone(),
                rx_audio_decoder,
            ) => panic!("Audio exit {:#?}", ret),
    );
}
