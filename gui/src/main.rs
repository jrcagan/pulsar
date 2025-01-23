use eframe::{egui::CentralPanel, run_native, App, NativeOptions};
use synth::{Oscillator, WaveType};
use rodio::{OutputStream, Sink};
use atomic_float::AtomicF32;
use std::sync::{Arc, atomic::Ordering};

struct Pulsar {
    oscillator: Oscillator,
    sink: Option<Sink>,
    _stream: Option<OutputStream>,
    frequency: Arc<AtomicF32>,
    silence: Arc<AtomicF32>
}

impl Default for Pulsar {
    fn default() -> Self {
        let freq = Arc::new(AtomicF32::new(440.0));
        let silence = Arc::new(AtomicF32::new(1.0));
        Self {
            oscillator: Oscillator::new(freq.clone(), 44100, WaveType::Sine, silence.clone()),
            sink: None,
            _stream: None,
            frequency: freq,
            silence
        }
    }
}

impl Pulsar {
    fn new() -> Self {
        Self::default()
    }

    fn start_sound(&mut self) {
        if self.sink.is_some() {
            return;
        }

        let (stream, stream_handle) = OutputStream::try_default().unwrap();
        let sink = Sink::try_new(&stream_handle).unwrap();
        let oscillator = Oscillator::new(self.frequency.clone(), 44100, WaveType::Sine, self.silence.clone());

        sink.append(oscillator);
        sink.play();

        self.sink = Some(sink);
        self._stream = Some(stream);
    }

    fn stop_sound(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self._stream.take();
    }
}

impl App for Pulsar {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            ui.label("Synth GUI");
            
            if ui.button("Play").clicked() {
                self.start_sound();
            }
            
            if ui.button("Stop").clicked() {
                self.stop_sound();
            }

            let mut curr_freq = self.frequency.load(Ordering::Relaxed);
            let mut curr_silence = self.silence.load(Ordering::Relaxed);

            if ui.add(eframe::egui::Slider::new(&mut curr_freq, 20.0..=2000.0).text("Frequency")).changed() {
                self.frequency.store(curr_freq, Ordering::Relaxed);
            }

            if ui.add(eframe::egui::Slider::new(&mut curr_silence, 0.0..=1.0).text("Silence")).changed() {
                self.silence.store(curr_silence, Ordering::Relaxed);
            }
        });
    }
}

fn main() {
    let app = Pulsar::new();
    let win_option = NativeOptions::default();
    run_native("Pulsar", win_option, Box::new(|_cc| Ok(Box::new(app))));
}