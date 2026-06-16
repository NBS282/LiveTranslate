use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;

/// Number of samples to buffer for a given latency in ms, sample rate, and channel count.
pub fn latency_samples(latency_ms: f32, sample_rate: u32, channels: u16) -> usize {
    let frames = (latency_ms / 1000.0) * sample_rate as f32;
    frames as usize * channels as usize
}

/// Holds the two live streams. Dropping this stops audio.
pub struct Passthrough {
    _input_stream: Stream,
    _output_stream: Stream,
}

/// Starts capturing from the default input device and writing to the
/// output device whose name matches `output_name`. Returns the live handle.
pub fn start(output_name: &str) -> Result<Passthrough, String> {
    let host = cpal::default_host();

    let input_device = host
        .default_input_device()
        .ok_or("no default input device")?;
    let output_device = host
        .output_devices()
        .map_err(|e| e.to_string())?
        .find(|d| d.name().map(|n| n == output_name).unwrap_or(false))
        .ok_or_else(|| format!("output device not found: {output_name}"))?;

    let config: StreamConfig = input_device
        .default_input_config()
        .map_err(|e| e.to_string())?
        .into();

    let buf = latency_samples(150.0, config.sample_rate.0, config.channels);
    let ring = HeapRb::<f32>::new(buf * 2);
    let (mut producer, mut consumer) = ring.split();
    // Pre-fill with silence to absorb device desync.
    for _ in 0..buf {
        let _ = producer.try_push(0.0);
    }

    let input_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        producer.push_slice(data);
    };
    let output_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let read = consumer.pop_slice(data);
        data[read..].fill(0.0);
    };
    let err_fn = |e| eprintln!("stream error: {e}");

    let input_stream = input_device
        .build_input_stream(&config, input_fn, err_fn, None)
        .map_err(|e| e.to_string())?;
    let output_stream = output_device
        .build_output_stream(&config, output_fn, err_fn, None)
        .map_err(|e| e.to_string())?;

    input_stream.play().map_err(|e| e.to_string())?;
    output_stream.play().map_err(|e| e.to_string())?;

    Ok(Passthrough {
        _input_stream: input_stream,
        _output_stream: output_stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_latency_samples() {
        // 100ms at 48kHz stereo = 0.1 * 48000 * 2 = 9600
        assert_eq!(latency_samples(100.0, 48_000, 2), 9_600);
    }

    #[test]
    fn mono_half_of_stereo() {
        assert_eq!(latency_samples(100.0, 48_000, 1), 4_800);
    }
}
