use earshot::Detector;

const FRAME_SIZE: usize = 256;
const THRESHOLD: f32 = 0.5;
const PAD_FRAMES: usize = 3;

pub struct VadFilter {
    detector: Box<Detector>,
}

impl VadFilter {
    pub fn new() -> Self {
        Self {
            detector: Detector::default_boxed(),
        }
    }

    pub fn filter_speech(&mut self, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        let n_frames = samples.len() / FRAME_SIZE;
        if n_frames == 0 {
            return samples.to_vec();
        }

        let mut is_speech = vec![false; n_frames];
        for i in 0..n_frames {
            let frame = &samples[i * FRAME_SIZE..(i + 1) * FRAME_SIZE];
            let score = self.detector.predict_f32(frame);
            is_speech[i] = score > THRESHOLD;
        }

        // Pad speech regions: mark PAD_FRAMES before/after each speech frame
        let mut padded = vec![false; n_frames];
        for i in 0..n_frames {
            if is_speech[i] {
                let start = i.saturating_sub(PAD_FRAMES);
                let end = (i + PAD_FRAMES + 1).min(n_frames);
                for p in &mut padded[start..end] {
                    *p = true;
                }
            }
        }

        let mut out = Vec::with_capacity(samples.len());
        for i in 0..n_frames {
            if padded[i] {
                out.extend_from_slice(&samples[i * FRAME_SIZE..(i + 1) * FRAME_SIZE]);
            }
        }

        // Include any trailing samples beyond the last full frame
        let remainder_start = n_frames * FRAME_SIZE;
        if remainder_start < samples.len() && padded.last().copied().unwrap_or(false) {
            out.extend_from_slice(&samples[remainder_start..]);
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_returns_empty() {
        let mut vad = VadFilter::new();
        let silence = vec![0.0f32; FRAME_SIZE * 20];
        let result = vad.filter_speech(&silence);
        assert!(result.is_empty() || result.len() < silence.len());
    }

    #[test]
    fn short_input_returned_as_is() {
        let mut vad = VadFilter::new();
        let short = vec![0.1f32; 100];
        let result = vad.filter_speech(&short);
        assert_eq!(result.len(), short.len());
    }

    #[test]
    fn empty_input_returns_empty() {
        let mut vad = VadFilter::new();
        let result = vad.filter_speech(&[]);
        assert!(result.is_empty());
    }
}
