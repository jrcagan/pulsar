use std::time::Duration;
use std::f32::consts::PI;
use std::sync::atomic::Ordering;
use atomic_float::AtomicF32;
use std::sync::Arc;
use rodio::Source;

#[derive(Default)]
#[derive(Clone)]
pub enum WaveType {
   Saw,
   Square,
   #[default] Sine,
   Triangle
}

#[derive(Clone)]
pub struct Oscillator {
   pub freq: Arc<AtomicF32>,
   number_of_samples: usize,
   sample_rate: u32,
   wave_type: WaveType,
   silence: Arc<AtomicF32>,
}

impl Oscillator {
   pub fn new(freq: Arc<AtomicF32>, sample_rate: u32, wave_type: WaveType, silence: Arc<AtomicF32>) -> Oscillator {
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
         wave_type: WaveType::Sine,
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
      self.number_of_samples = self.number_of_samples.wrapping_add(1);

      value = 2.0 * (freq * self.number_of_samples as f32 / self.sample_rate as f32 % 1.0) - 1.0;
      
      let val_with_silence = value * (1.0 / silence);

      // Creates areas of silence in the pulset 
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

