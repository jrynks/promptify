/// Sample rate expected by the speech-to-text engines.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    /// True when the recording hit the length cap and later audio was dropped.
    pub capped: bool,
}

/// Converts interleaved device audio of any rate/channel count into bounded mono 16 kHz.
#[derive(Debug)]
pub struct CaptureBuffer {
    channels: usize,
    step: f64,
    max_samples: usize,
    samples: Vec<f32>,
    prev: Option<f32>,
    in_index: u64,
    next_out: f64,
    capped: bool,
}

impl CaptureBuffer {
    pub fn new(channels: u16, source_rate: u32, max_samples: usize) -> Self {
        assert!(channels > 0 && source_rate > 0, "invalid audio format");
        Self {
            channels: channels as usize,
            step: f64::from(source_rate) / f64::from(TARGET_SAMPLE_RATE),
            max_samples,
            samples: Vec::with_capacity(max_samples.min(TARGET_SAMPLE_RATE as usize * 30)),
            prev: None,
            in_index: 0,
            next_out: 0.0,
            capped: false,
        }
    }

    /// Appends interleaved frames and returns the chunk's RMS level (0..=1) for a level meter.
    pub fn push_interleaved(&mut self, data: &[f32]) -> f32 {
        let mut sum_sq = 0.0f64;
        let mut frames = 0usize;
        for frame in data.chunks_exact(self.channels) {
            let mono = frame.iter().sum::<f32>() / self.channels as f32;
            sum_sq += f64::from(mono) * f64::from(mono);
            frames += 1;
            self.push_mono(mono);
        }
        if frames == 0 { 0.0 } else { (sum_sq / frames as f64).sqrt().min(1.0) as f32 }
    }

    fn push_mono(&mut self, x: f32) {
        let current = self.in_index as f64;
        match self.prev {
            None => {
                while self.next_out <= current {
                    self.emit(x);
                    self.next_out += self.step;
                }
            }
            Some(prev) => {
                while self.next_out <= current {
                    let frac = (self.next_out - (current - 1.0)) as f32;
                    self.emit(prev + (x - prev) * frac);
                    self.next_out += self.step;
                }
            }
        }
        self.prev = Some(x);
        self.in_index += 1;
    }

    fn emit(&mut self, sample: f32) {
        if self.samples.len() >= self.max_samples {
            self.capped = true;
        } else {
            self.samples.push(sample);
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn finish(self) -> CapturedAudio {
        CapturedAudio { samples: self.samples, capped: self.capped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_target_rate() {
        let mut buf = CaptureBuffer::new(1, 16_000, 100);
        buf.push_interleaved(&[0.1, 0.2, 0.3]);
        assert_eq!(buf.finish().samples, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn downsamples_48k_to_16k() {
        let mut buf = CaptureBuffer::new(1, 48_000, usize::MAX);
        let input: Vec<f32> = (0..4800).map(|i| i as f32).collect();
        buf.push_interleaved(&input);
        let out = buf.finish().samples;
        assert_eq!(out.len(), 1600);
        assert_eq!(out[1], 3.0);
    }

    #[test]
    fn upsamples_8k_with_interpolation() {
        let mut buf = CaptureBuffer::new(1, 8_000, usize::MAX);
        buf.push_interleaved(&[0.0, 1.0]);
        assert_eq!(buf.finish().samples, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn downmixes_channels_and_reports_level() {
        let mut buf = CaptureBuffer::new(2, 16_000, 10);
        let level = buf.push_interleaved(&[0.5, -0.5, 1.0, 0.0]);
        assert_eq!(buf.finish().samples, vec![0.0, 0.5]);
        assert!((level - (0.125f32).sqrt()).abs() < 1e-6);
    }

    #[test]
    fn caps_length_and_reports_it() {
        let mut buf = CaptureBuffer::new(1, 16_000, 2);
        buf.push_interleaved(&[0.1, 0.2, 0.3]);
        assert_eq!(buf.finish(), CapturedAudio { samples: vec![0.1, 0.2], capped: true });
    }
}
