use atomic_float::AtomicF32;
use rodio::Source;
use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Waves
// ---------------------------------------------------------------------------

/// The sound-generating waves, in the order the Wave control morphs through them.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum WaveType {
   #[default] Sine,
   Triangle,
   Saw,
   Square,
   /// A few stacked harmonics, like organ drawbars
   Organ,
   /// A sine whose phase is pushed around by a faster sine
   Fm,
   /// A sine driven hard enough to fold back on itself
   Fold,
   /// A decaying burst inside each cycle, giving a vowel-like tone
   Vox,
   Noise,
}

impl WaveType {
   pub const ALL: [WaveType; 9] = [
      WaveType::Sine,
      WaveType::Triangle,
      WaveType::Saw,
      WaveType::Square,
      WaveType::Organ,
      WaveType::Fm,
      WaveType::Fold,
      WaveType::Vox,
      WaveType::Noise,
   ];

   pub fn name(&self) -> &'static str {
      match self {
         WaveType::Sine => "Sine",
         WaveType::Triangle => "Tri",
         WaveType::Saw => "Saw",
         WaveType::Square => "Square",
         WaveType::Organ => "Organ",
         WaveType::Fm => "FM",
         WaveType::Fold => "Fold",
         WaveType::Vox => "Vox",
         WaveType::Noise => "Noise",
      }
   }
}

/// Highest position of the Wave control (one whole step per wave).
pub const MAX_SHAPE: f32 = (WaveType::ALL.len() - 1) as f32;

/// One wave at `phase` (measured in cycles), in -1.0..=1.0.
/// Noise has no phase, so it returns 0.0 here; callers supply their own randomness.
pub fn wave_value(wave_type: &WaveType, phase: f32) -> f32 {
   let p = phase.rem_euclid(1.0);
   match wave_type {
      WaveType::Sine => (TAU * p).sin(),
      WaveType::Triangle => 2.0 / PI * (TAU * p).sin().asin(),
      WaveType::Saw => 2.0 * p - 1.0,
      WaveType::Square => if p < 0.5 { 1.0 } else { -1.0 },
      WaveType::Organ => {
         ((TAU * p).sin()
            + 0.5 * (2.0 * TAU * p).sin()
            + 0.35 * (3.0 * TAU * p).sin()
            + 0.25 * (4.0 * TAU * p).sin()
            + 0.2 * (6.0 * TAU * p).sin()
            + 0.15 * (8.0 * TAU * p).sin())
            / 1.46
      }
      WaveType::Fm => (TAU * p + 1.8 * (2.0 * TAU * p).sin()).sin(),
      WaveType::Fold => (5.5 * (TAU * p).sin()).sin(),
      WaveType::Vox => (5.5 * TAU * p).sin() * (1.0 - p) * (1.0 - p) / 0.92,
      WaveType::Noise => 0.0,
   }
}

/// The blend between neighbouring waves at Wave position `shape`
/// (0.0 = first wave, 1.0 = second, 1.5 = halfway between second and third...).
/// `noise` is the random value to use for the Noise wave.
pub fn morph_value(shape: f32, phase: f32, noise: f32) -> f32 {
   let shape = shape.clamp(0.0, MAX_SHAPE);
   let index = (shape.floor() as usize).min(WaveType::ALL.len() - 1);
   let blend = shape - index as f32;
   let one = |i: usize| match WaveType::ALL[i] {
      WaveType::Noise => noise,
      wave => wave_value(&wave, phase),
   };
   if blend <= 0.0 || index + 1 >= WaveType::ALL.len() {
      one(index)
   } else {
      one(index) * (1.0 - blend) + one(index + 1) * blend
   }
}

/// The silence distortion: boost the wave by 1/silence, then drop anything
/// that ends up above 1.0 to silence.
pub fn apply_silence(value: f32, silence: f32) -> f32 {
   let val_with_silence = value * (1.0 / silence);
   if val_with_silence > 1.0 { 0.0 } else { val_with_silence }
}

// ---------------------------------------------------------------------------
// LFO
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum LfoShape {
   #[default] Sine,
   Triangle,
   Saw,
   Square,
   /// Jumps to a new random level once per cycle
   Random,
}

impl LfoShape {
   pub const ALL: [LfoShape; 5] = [
      LfoShape::Sine,
      LfoShape::Triangle,
      LfoShape::Saw,
      LfoShape::Square,
      LfoShape::Random,
   ];

   pub fn name(&self) -> &'static str {
      match self {
         LfoShape::Sine => "Sine",
         LfoShape::Triangle => "Triangle",
         LfoShape::Saw => "Saw",
         LfoShape::Square => "Square",
         LfoShape::Random => "Random",
      }
   }

   fn from_u8(value: u8) -> LfoShape {
      LfoShape::ALL[(value as usize).min(LfoShape::ALL.len() - 1)]
   }
}

/// The LFO at `phase` (in cycles), in -1.0..=1.0.
/// Random returns 0.0 here; the voice holds its current random level itself.
pub fn lfo_value(shape: &LfoShape, phase: f32) -> f32 {
   let p = phase.rem_euclid(1.0);
   match shape {
      LfoShape::Sine => (TAU * p).sin(),
      LfoShape::Triangle => 2.0 / PI * (TAU * p).sin().asin(),
      LfoShape::Saw => 2.0 * p - 1.0,
      LfoShape::Square => if p < 0.5 { 1.0 } else { -1.0 },
      LfoShape::Random => 0.0,
   }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

/// How much lowpass, bandpass and highpass to mix for a Type setting of
/// 0.0 (lowpass) through 0.5 (bandpass) to 1.0 (highpass).
pub fn filter_mix(morph: f32) -> (f32, f32, f32) {
   let morph = morph.clamp(0.0, 1.0);
   if morph < 0.5 {
      let t = morph * 2.0;
      (1.0 - t, t, 0.0)
   } else {
      let t = (morph - 0.5) * 2.0;
      (0.0, 1.0 - t, t)
   }
}

/// Resonance setting (0.0..=1.0) to filter Q (0.5..=20).
fn resonance_to_q(resonance: f32) -> f32 {
   0.5 * 40.0_f32.powf(resonance.clamp(0.0, 1.0))
}

/// How loud a tone at `freq` comes out of the filter (1.0 = unchanged).
/// Used to draw the filter curve.
pub fn filter_response(freq: f32, cutoff: f32, resonance: f32, morph: f32) -> f32 {
   let (low, band, high) = filter_mix(morph);
   let k = 1.0 / resonance_to_q(resonance);
   let x = freq / cutoff;
   let (num_re, num_im) = (low - high * x * x, band * k * x);
   let (den_re, den_im) = (1.0 - x * x, k * x);
   (num_re.hypot(num_im)) / (den_re.hypot(den_im)).max(1e-9)
}

// ---------------------------------------------------------------------------
// Settings shared between the UI and the audio thread
// ---------------------------------------------------------------------------

/// A plain snapshot of every control.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
   pub frequency: f32,
   /// Wave position, 0.0..=MAX_SHAPE
   pub shape: f32,
   pub silence: f32,
   /// 0.0..=1.0
   pub volume: f32,
   pub cutoff: f32,
   /// 0.0..=1.0
   pub resonance: f32,
   /// 0.0 lowpass, 0.5 bandpass, 1.0 highpass
   pub filter_morph: f32,
   pub lfo_shape: LfoShape,
   /// Hz
   pub lfo_rate: f32,
   /// How far the LFO pushes the Wave position either way, in waves
   pub lfo_depth: f32,
}

impl Default for Settings {
   fn default() -> Self {
      Settings {
         frequency: 440.0,
         shape: 0.0,
         silence: 1.0,
         volume: 0.35,
         cutoff: 18000.0,
         resonance: 0.1,
         filter_morph: 0.0,
         lfo_shape: LfoShape::Sine,
         lfo_rate: 1.0,
         lfo_depth: 0.0,
      }
   }
}

/// The live controls. The UI writes them, the audio thread reads them.
pub struct Params {
   pub frequency: AtomicF32,
   pub shape: AtomicF32,
   pub silence: AtomicF32,
   pub volume: AtomicF32,
   pub cutoff: AtomicF32,
   pub resonance: AtomicF32,
   pub filter_morph: AtomicF32,
   lfo_shape: AtomicU8,
   pub lfo_rate: AtomicF32,
   pub lfo_depth: AtomicF32,
   /// Written by the audio thread: where the Wave position is right now,
   /// LFO included.
   pub shape_now: AtomicF32,
}

impl Default for Params {
   fn default() -> Self {
      let s = Settings::default();
      Params {
         frequency: AtomicF32::new(s.frequency),
         shape: AtomicF32::new(s.shape),
         silence: AtomicF32::new(s.silence),
         volume: AtomicF32::new(s.volume),
         cutoff: AtomicF32::new(s.cutoff),
         resonance: AtomicF32::new(s.resonance),
         filter_morph: AtomicF32::new(s.filter_morph),
         lfo_shape: AtomicU8::new(s.lfo_shape as u8),
         lfo_rate: AtomicF32::new(s.lfo_rate),
         lfo_depth: AtomicF32::new(s.lfo_depth),
         shape_now: AtomicF32::new(s.shape),
      }
   }
}

impl Params {
   pub fn lfo_shape(&self) -> LfoShape {
      LfoShape::from_u8(self.lfo_shape.load(Ordering::Relaxed))
   }

   pub fn set_lfo_shape(&self, shape: LfoShape) {
      self.lfo_shape.store(shape as u8, Ordering::Relaxed);
   }

   pub fn load(&self) -> Settings {
      let o = Ordering::Relaxed;
      Settings {
         frequency: self.frequency.load(o),
         shape: self.shape.load(o),
         silence: self.silence.load(o),
         volume: self.volume.load(o),
         cutoff: self.cutoff.load(o),
         resonance: self.resonance.load(o),
         filter_morph: self.filter_morph.load(o),
         lfo_shape: self.lfo_shape(),
         lfo_rate: self.lfo_rate.load(o),
         lfo_depth: self.lfo_depth.load(o),
      }
   }
}

// ---------------------------------------------------------------------------
// The voice: oscillator -> silence distortion -> filter -> volume
// ---------------------------------------------------------------------------

/// What one sample looked like at each stage.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
   /// The wave straight out of the oscillator
   pub raw: f32,
   /// After the silence distortion and the filter, before volume
   pub filtered: f32,
   /// What goes to the speakers
   pub out: f32,
   /// The Wave position used for this sample, LFO included
   pub shape: f32,
}

/// All the sound-making state. The UI also runs one of these to draw the scope,
/// so what you see is produced by the same code as what you hear.
#[derive(Clone)]
pub struct Voice {
   sample_rate: f32,
   phase: f32,
   lfo_phase: f32,
   lfo_hold: f32,
   rng: u32,
   /// The controls as currently applied; they glide toward the targets to avoid clicks.
   now: Settings,
   gain: f32,
   glide: f32,
   // filter state and the settings its coefficients were worked out for
   ic1: f32,
   ic2: f32,
   g: f32,
   k: f32,
   coef_cutoff: f32,
   coef_resonance: f32,
}

impl Voice {
   pub fn new(sample_rate: u32) -> Voice {
      let sample_rate = sample_rate as f32;
      let mut voice = Voice {
         sample_rate,
         phase: 0.0,
         lfo_phase: 0.0,
         lfo_hold: 0.0,
         rng: 0x2545_F491,
         now: Settings::default(),
         // Starts silent and glides up, so starting playback fades in.
         gain: 0.0,
         // Reach a new knob position in roughly 5 ms
         glide: 1.0 - (-1.0 / (0.005 * sample_rate)).exp(),
         ic1: 0.0,
         ic2: 0.0,
         g: 0.0,
         k: 0.0,
         coef_cutoff: -1.0,
         coef_resonance: -1.0,
      };
      voice.update_filter();
      voice
   }

   /// Jump straight to these settings with no glide.
   pub fn snap(&mut self, settings: &Settings) {
      self.now = *settings;
      self.gain = settings.volume * settings.volume;
   }

   pub fn set_phase(&mut self, phase: f32) {
      self.phase = phase.rem_euclid(1.0);
   }

   pub fn seed(&mut self, seed: u32) {
      self.rng = seed | 1;
   }

   /// A random value in -1.0..1.0
   fn random(&mut self) -> f32 {
      self.rng ^= self.rng << 13;
      self.rng ^= self.rng >> 17;
      self.rng ^= self.rng << 5;
      (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
   }

   fn update_filter(&mut self) {
      let cutoff = self.now.cutoff.clamp(10.0, self.sample_rate * 0.45);
      self.g = (PI * cutoff / self.sample_rate).tan();
      self.k = 1.0 / resonance_to_q(self.now.resonance);
      self.coef_cutoff = self.now.cutoff;
      self.coef_resonance = self.now.resonance;
   }

   pub fn tick(&mut self, target: &Settings) -> Frame {
      // Glide the controls toward where the knobs are.
      let a = self.glide;
      let now = &mut self.now;
      now.frequency += a * (target.frequency - now.frequency);
      now.shape += a * (target.shape - now.shape);
      now.silence += a * (target.silence - now.silence);
      now.cutoff += a * (target.cutoff - now.cutoff);
      now.resonance += a * (target.resonance - now.resonance);
      now.filter_morph += a * (target.filter_morph - now.filter_morph);
      now.lfo_depth += a * (target.lfo_depth - now.lfo_depth);
      now.lfo_rate = target.lfo_rate;
      now.lfo_shape = target.lfo_shape;
      now.volume = target.volume;
      self.gain += a * (target.volume * target.volume - self.gain);

      // LFO
      self.lfo_phase += self.now.lfo_rate / self.sample_rate;
      if self.lfo_phase >= 1.0 {
         self.lfo_phase -= self.lfo_phase.floor();
         self.lfo_hold = self.random();
      }
      let lfo = match self.now.lfo_shape {
         LfoShape::Random => self.lfo_hold,
         shape => lfo_value(&shape, self.lfo_phase),
      };
      let shape = (self.now.shape + lfo * self.now.lfo_depth).clamp(0.0, MAX_SHAPE);

      // Oscillator
      self.phase += self.now.frequency / self.sample_rate;
      self.phase -= self.phase.floor();
      let noise = if shape > MAX_SHAPE - 1.0 { self.random() } else { 0.0 };
      let raw = morph_value(shape, self.phase, noise);

      // Silence distortion. The clamp is what the sound card would do anyway,
      // and keeps infinities out of the filter.
      let distorted = apply_silence(raw, self.now.silence.max(0.001)).clamp(-1.0, 1.0);

      // Filter (a state-variable filter gives low, band and high outputs at once)
      if (self.now.cutoff - self.coef_cutoff).abs() > self.coef_cutoff.abs() * 1e-4
         || (self.now.resonance - self.coef_resonance).abs() > 1e-4
      {
         self.update_filter();
      }
      let (g, k) = (self.g, self.k);
      let a1 = 1.0 / (1.0 + g * (g + k));
      let a2 = g * a1;
      let a3 = g * a2;
      let v3 = distorted - self.ic2;
      let v1 = a1 * self.ic1 + a2 * v3;
      let v2 = self.ic2 + a2 * self.ic1 + a3 * v3;
      self.ic1 = 2.0 * v1 - self.ic1;
      self.ic2 = 2.0 * v2 - self.ic2;
      let (low, band, high) = (v2, k * v1, distorted - k * v1 - v2);
      let (mix_low, mix_band, mix_high) = filter_mix(self.now.filter_morph);
      let mut filtered = low * mix_low + band * mix_band + high * mix_high;
      if !filtered.is_finite() {
         self.ic1 = 0.0;
         self.ic2 = 0.0;
         filtered = 0.0;
      }

      Frame {
         raw,
         filtered,
         out: (filtered * self.gain).clamp(-1.0, 1.0),
         shape,
      }
   }
}

// ---------------------------------------------------------------------------
// Audio source
// ---------------------------------------------------------------------------

/// Plays a `Voice` using the live `Params`.
pub struct Oscillator {
   params: Arc<Params>,
   settings: Settings,
   voice: Voice,
   sample_rate: u32,
   until_refresh: u32,
}

impl Oscillator {
   pub fn new(params: Arc<Params>, sample_rate: u32) -> Oscillator {
      let settings = params.load();
      let mut voice = Voice::new(sample_rate);
      // Start from the current knob positions, but leave the volume at zero to fade in.
      voice.snap(&Settings { volume: 0.0, ..settings });
      Oscillator { params, settings, voice, sample_rate, until_refresh: 0 }
   }
}

impl Iterator for Oscillator {
   type Item = f32;

   fn next(&mut self) -> Option<f32> {
      let refresh = self.until_refresh == 0;
      if refresh {
         self.settings = self.params.load();
         self.until_refresh = 16;
      }
      self.until_refresh -= 1;

      let frame = self.voice.tick(&self.settings);
      if refresh {
         self.params.shape_now.store(frame.shape, Ordering::Relaxed);
      }
      Some(frame.out)
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

#[cfg(test)]
mod tests {
   use super::*;

   fn run(settings: Settings, samples: usize) -> Vec<Frame> {
      let mut voice = Voice::new(44100);
      voice.snap(&settings);
      (0..samples).map(|_| voice.tick(&settings)).collect()
   }

   fn peak(frames: &[Frame], pick: fn(&Frame) -> f32) -> f32 {
      frames.iter().map(|f| pick(f).abs()).fold(0.0, f32::max)
   }

   #[test]
   fn every_wave_stays_in_range() {
      for wave in WaveType::ALL {
         for i in 0..2000 {
            let v = wave_value(&wave, i as f32 / 2000.0);
            assert!(v.abs() <= 1.001, "{wave:?} reached {v}");
         }
      }
   }

   #[test]
   fn output_is_finite_and_bounded_at_extreme_settings() {
      for shape in 0..=8 {
         for &(silence, resonance, morph) in &[(0.0, 1.0, 0.0), (1.0, 1.0, 0.5), (0.3, 0.0, 1.0)] {
            let settings = Settings {
               shape: shape as f32,
               silence,
               resonance,
               filter_morph: morph,
               cutoff: 500.0,
               volume: 1.0,
               lfo_depth: 4.0,
               lfo_rate: 20.0,
               lfo_shape: LfoShape::Random,
               ..Settings::default()
            };
            for frame in run(settings, 20000) {
               assert!(frame.out.is_finite() && frame.out.abs() <= 1.0);
               assert!(frame.filtered.is_finite());
            }
         }
      }
   }

   #[test]
   fn volume_scales_the_output() {
      let quiet = run(Settings { volume: 0.2, ..Settings::default() }, 4000);
      let off = run(Settings { volume: 0.0, ..Settings::default() }, 4000);
      let p = peak(&quiet, |f| f.out);
      assert!((p - 0.04).abs() < 0.005, "peak was {p}");
      assert_eq!(peak(&off, |f| f.out), 0.0);
   }

   #[test]
   fn filter_types_pass_the_right_frequencies() {
      // A 200 Hz sine against a 2 kHz cutoff: lowpass keeps it, highpass removes it.
      let base = Settings { frequency: 200.0, cutoff: 2000.0, ..Settings::default() };
      let low = run(Settings { filter_morph: 0.0, ..base }, 8000);
      let high = run(Settings { filter_morph: 1.0, ..base }, 8000);
      assert!(peak(&low[4000..], |f| f.filtered) > 0.9);
      assert!(peak(&high[4000..], |f| f.filtered) < 0.05);

      // The drawn curve agrees with the real filter for a tone at the cutoff in bandpass.
      let band = run(Settings { frequency: 2000.0, filter_morph: 0.5, ..base }, 8000);
      let measured = peak(&band[4000..], |f| f.filtered);
      let drawn = filter_response(2000.0, 2000.0, base.resonance, 0.5);
      assert!((measured - drawn).abs() < 0.05, "measured {measured}, drawn {drawn}");
   }

   #[test]
   fn lfo_moves_the_wave_position() {
      let settings = Settings { shape: 4.0, lfo_depth: 2.0, lfo_rate: 10.0, ..Settings::default() };
      let frames = run(settings, 44100);
      let lowest = frames.iter().map(|f| f.shape).fold(f32::MAX, f32::min);
      let highest = frames.iter().map(|f| f.shape).fold(f32::MIN, f32::max);
      assert!(lowest < 2.1 && highest > 5.9, "moved between {lowest} and {highest}");
   }
}
