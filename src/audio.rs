pub mod input;
pub mod output;

use crate::VoiceStreamType;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, StreamConfig};
use futures_util::StreamExt;
use log::*;
use mumble_protocol::voice::{VoicePacket, VoicePacketPayload};
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, HashMap},
};
use tokio::time::Instant;

use crate::config::Config;
use crate::{RxAudioDecoder, StreamType, TxCommandPacket, TxSenderPacket};
use cpal::{InputCallbackInfo, Sample};
use mumble_protocol::control::ControlPacket;
use tokio::select;

const SAMPLE_RATE: u32 = 48000;

#[derive(Clone, Debug)]
pub enum AudioCommand {
    AddClient(u32),
    DelClient(u32),
    Voice(VoiceStreamType, u32, VoicePacketPayload),
}

fn input_callback<T: Sample>(
    mut sender: futures_channel::mpsc::Sender<Vec<f32>>,
    size: usize,
) -> impl FnMut(&[T], &InputCallbackInfo) + Send + 'static {
    let mut buffer = Vec::with_capacity(size);
    debug!("Input Callback");
    move |data: &[T], _info: &InputCallbackInfo| {
        for sample in data.iter().map(|e| e.to_f32()) {
            if buffer.len() == size {
                if let Err(_e) = sender.try_send(buffer.clone()) {
                    warn!("Error sending audio: {}", _e);
                }
                buffer.clear();
            }
            buffer.push(sample);
        }
    }
}

pub async fn handle(
    config: Arc<Config>,
    stream_type: StreamType,
    tx_command: TxCommandPacket,
    tx_sender: TxSenderPacket,
    mut rx_audio_decoder: RxAudioDecoder,
) -> crate::Result<()> {
    select!(
        ret = decoder(config, &mut rx_audio_decoder) => ret,
        ret = encoder(stream_type, tx_command, tx_sender) => ret,
    )
}

pub async fn encoder(
    stream_type: StreamType,
    tx_tcp: TxCommandPacket,
    tx_udp: TxSenderPacket,
) -> crate::Result<()> {
    let sample_rate = SampleRate(SAMPLE_RATE);

    let host = cpal::default_host();
    let input_device = host.default_input_device().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "default input device not found",
        )
    })?;
    let input_supported_config = input_device
        .supported_input_configs()?
        .find_map(|c| {
            if c.min_sample_rate() <= sample_rate
                && c.max_sample_rate() >= sample_rate
            {
                Some(c)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Unable to receive from stream",
            )
        })?
        .with_sample_rate(sample_rate);
    let input_supported_sample_format = input_supported_config.sample_format();
    let input_config: StreamConfig = input_supported_config.into();

    let err_fn =
        |err| error!("An error occurred on the output audio stream: {}", err);

    let (tx_audio_input, mut rx_audio_input) =
        futures_channel::mpsc::channel(1_000_000);

    let frame_size = 4;
    let opus_frame_size =
        (frame_size * input_config.sample_rate.0 / 400) as usize;
    let input_stream = match input_supported_sample_format {
        SampleFormat::F32 => input_device.build_input_stream(
            &input_config,
            input_callback::<f32>(tx_audio_input, opus_frame_size),
            err_fn,
        ),
        SampleFormat::I16 => input_device.build_input_stream(
            &input_config,
            input_callback::<i16>(tx_audio_input, opus_frame_size),
            err_fn,
        ),
        SampleFormat::U16 => input_device.build_input_stream(
            &input_config,
            input_callback::<u16>(tx_audio_input, opus_frame_size),
            err_fn,
        ),
    }?;
    input_stream.play()?;

    let mut opus_encoder = opus::Encoder::new(
        input_config.sample_rate.0,
        match input_config.channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => unimplemented!(
                "Only 1 or 2 channels supported, got {})",
                input_config.channels,
            ),
        },
        opus::Application::Voip,
    )?;

    let mut packet_seq_num = 0u64;
    let mut highest_sound = 0f32;
    let mut last_sound = Instant::now();
    loop {
        let audio_frame = rx_audio_input.next().await.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Unable to receive from stream",
            )
        })?;
        let stream_type = stream_type
            .lock()
            .or_else(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "audio encoder, unable to lock stream_type",
                ))
            })?
            .clone();
        match stream_type {
            None => {
                //not connected yet, just consume the audio
            }
            Some(x) => {
                //check is is silence
                let mut loud_sound = false;
                for x in audio_frame.iter() {
                   if !loud_sound {
                       if let Some(Ordering::Less) = (highest_sound * 0.1).partial_cmp(x) {
                           loud_sound = true;
                       }
                   }
                   if let Some(Ordering::Less) = highest_sound.partial_cmp(x) {
                       highest_sound = *x;
                   }
                }
                if loud_sound {
                    last_sound = Instant::now();
                }
                //encode and send if received recently or have sound right now
                if loud_sound || last_sound.elapsed().as_millis() < 1000 {
                    let encoded = opus_encoder
                        .encode_vec_float(&audio_frame, opus_frame_size)?;
                    let packet = VoicePacket::Audio {
                        _dst: std::marker::PhantomData,
                        target: 0,      // normal speech
                        session_id: (), // unused for server-bound packets
                        seq_num: packet_seq_num,
                        payload: VoicePacketPayload::Opus(encoded.into(), false),
                        position_info: None,
                    };
                    match x {
                        VoiceStreamType::UDP => {
                            tx_tcp
                                .send(ControlPacket::UDPTunnel(Box::new(packet)))?;
                        }
                        VoiceStreamType::TCP => {
                            tx_udp.send(packet)?;
                        }
                    }
                    packet_seq_num = packet_seq_num.wrapping_add(1);
                }
            }
        }
    }
}

pub async fn decoder(
    config: Arc<Config>,
    rx_audio_decoder: &mut RxAudioDecoder,
) -> crate::Result<()> {
    let sample_rate = SampleRate(SAMPLE_RATE);

    let volume = config.output_volume;

    let host = cpal::default_host();
    let output_device = host.default_output_device().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "default output device not found",
        )
    })?;
    let output_supported_config = output_device
        .supported_output_configs()?
        .find_map(|c| {
            if c.min_sample_rate() <= sample_rate
                && c.max_sample_rate() >= sample_rate
            {
                Some(c)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "audio decoder, unable to get output config",
            )
        })?
        .with_sample_rate(sample_rate);
    let output_supported_sample_format =
        output_supported_config.sample_format();
    let output_config: StreamConfig = output_supported_config.into();

    let err_fn =
        |err| error!("An error occurred on the output audio stream: {}", err);

    let client_streams: Arc<
        Mutex<HashMap<(VoiceStreamType, u32), output::ClientStream>>,
    > = Arc::new(Mutex::new(HashMap::new()));
    let output_stream = match output_supported_sample_format {
        SampleFormat::F32 => output_device.build_output_stream(
            &output_config,
            output::callback::<f32>(Arc::clone(&client_streams), volume),
            err_fn,
        ),
        SampleFormat::I16 => output_device.build_output_stream(
            &output_config,
            output::callback::<i16>(Arc::clone(&client_streams), volume),
            err_fn,
        ),
        SampleFormat::U16 => output_device.build_output_stream(
            &output_config,
            output::callback::<u16>(Arc::clone(&client_streams), volume),
            err_fn,
        ),
    }?;

    //TODO How to handle this thread in tokio?
    output_stream.play()?;
    loop {
        let packet = rx_audio_decoder.recv().await.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Unable to receive from stream",
            )
        })?;
        match packet {
            AudioCommand::AddClient(id) => {
                for stream_type in
                    [VoiceStreamType::UDP, VoiceStreamType::TCP].iter()
                {
                    let mut client_streams =
                        client_streams.lock().or_else(|_| {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::ConnectionAborted,
                                "audio encoder, unable to lock client_stream",
                            ))
                        })?;
                    match client_streams.entry((*stream_type, id)) {
                        Entry::Occupied(_) => {
                            panic!("Session id {} already exists", id);
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(output::ClientStream::new(
                                output_config.sample_rate.0,
                                output_config.channels,
                            ));
                        }
                    }
                }
            }
            AudioCommand::DelClient(id) => {
                for stream_type in
                    [VoiceStreamType::TCP, VoiceStreamType::UDP].iter()
                {
                    let mut client_streams =
                        client_streams.lock().or_else(|_| {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::ConnectionAborted,
                                "audio encoder, unable to lock client_stream",
                            ))
                        })?;
                    match client_streams.entry((*stream_type, id)) {
                        Entry::Occupied(entry) => {
                            entry.remove();
                        }
                        Entry::Vacant(_) => {
                            panic!(
                                "Tried to remove session id {} that doesn't exist",
                                id
                            );
                        }
                    }
                }
            }
            AudioCommand::Voice(kind, id, payload) => {
                let mut client_streams =
                    client_streams.lock().or_else(|_| {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::ConnectionAborted,
                            "audio encoder, unable to lock client_stream",
                        ))
                    })?;
                match client_streams.entry((kind, id)) {
                    Entry::Occupied(mut entry) => {
                        entry.get_mut().decode_packet(
                            payload,
                            output_config.channels as usize,
                        );
                    }
                    Entry::Vacant(_) => {
                        panic!("Can't find session id {}", id);
                    }
                }
            }
        }
    }
}
