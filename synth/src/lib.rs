use std::time::Duration;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU8, Ordering};
use atomic_float::AtomicF32;
use std::sync::Arc;
use rodio::Source;
use rand::Rng;

#[derive(Default)]
#[derive(Clone)]
#[derive(PartialEq)]
pub enum WaveType {
   Saw,
   Noise,
   #[default] Sine,
   Triangle
}

#[derive(Clone)]
pub struct AtomicWaveType {
    inner: Arc<AtomicU8>,
}

impl AtomicWaveType {
    pub fn new(initial: WaveType) -> Self {
        Self {
            inner: Arc::new(AtomicU8::new(initial as u8)),
        }
    }

    pub fn load(&self, order: Ordering) -> WaveType {
        match self.inner.load(order) {
            0 => WaveType::Saw,
            1 => WaveType::Noise,
            2 => WaveType::Sine,
            _ => WaveType::Triangle,
        }
    }

    pub fn store(&self, wave_type: WaveType, order: Ordering) {
        self.inner.store(wave_type as u8, order);
    }
}

#[derive(Clone)]
pub struct Oscillator {
   freq: Arc<AtomicF32>,
   number_of_samples: usize,
   sample_rate: u32,
   pub wave_type: AtomicWaveType,
   silence: Arc<AtomicF32>,
}

impl Oscillator {
   pub fn new(freq: Arc<AtomicF32>, sample_rate: u32, wave_type: AtomicWaveType, silence: Arc<AtomicF32>) -> Oscillator {
      Oscillator {
         freq: freq,
         sample_rate: sample_rate,
         number_of_samples: 0,
         wave_type: wave_type,
         silence: silence
      }
   }

}

impl Default for Oscillator {
   fn default() -> Self {
      Oscillator {
         freq: Arc::new(AtomicF32::new(440.0)),
         sample_rate: 41000,
         number_of_samples: 0,
         wave_type: AtomicWaveType::new(WaveType::Sine),
         silence: Arc::new(AtomicF32::new(1.0))
      }
   }
}

impl Iterator for Oscillator {
   type Item = f32;

   fn next(&mut self) -> Option<f32> {
       let value;
       let freq = self.freq.load(Ordering::Relaxed);
       let silence = self.silence.load(Ordering::Relaxed);
       let wave_type = self.wave_type.load(Ordering::Relaxed);
       self.number_of_samples = self.number_of_samples.wrapping_add(1);
       let mut rndm = rand::thread_rng();

       value = match wave_type {
           WaveType::Saw => 2.0 * (freq * self.number_of_samples as f32 / self.sample_rate as f32 % 1.0) - 1.0,
           WaveType::Sine => (2.0 * PI * freq * (self.number_of_samples as f32 / self.sample_rate as f32)).sin(),
           WaveType::Triangle => 2.0 / PI
               * (2.0 * PI * freq * self.number_of_samples as f32 / self.sample_rate as f32)
                   .sin()
                   .asin(),
           WaveType::Noise => rndm.gen_range(-1.0..1.0)
       };

       let val_with_silence = value * (1.0 / silence);
       Some(if val_with_silence > 1.0 { 0.0 } else { val_with_silence })
   }
}

impl Source for Oscillator {
   fn channels(&self) -> u16 {
      1
   }

   fn sample_rate(&self) -> u32 {
      self.sample_rate
   }

   fn current_frame_len(&self) -> Option<usize> {
      None
   }

   fn total_duration(&self) -> Option<Duration> {
      None
   }
}

