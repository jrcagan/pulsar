use atomic_float::AtomicF32;
use eframe::{egui::CentralPanel, run_native, App, NativeOptions};
use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Pos2, Rect, Response, Sense, Shape,
    Stroke, Ui,
};
use rodio::{OutputStream, Sink};
use std::f32::consts::PI;
use std::path::Path;
use std::sync::{atomic::Ordering, Arc};
use synth::{
    filter_response, lfo_value, sample_position, wave_value, LfoShape, LfoTarget, Oscillator, Params, SampleData, Settings,
    Voice, WaveType, MAX_SHAPE,
};

const SAMPLE_RATE: u32 = 44100;
/// The sample that is built into the program and loaded at startup.
const DEFAULT_SAMPLE: &[u8] = include_bytes!("../assets/idmSynth.wav");
const DEFAULT_SAMPLE_NAME: &str = "idmSynth.wav";
/// How much time the scope shows, in seconds.
const SCOPE_WINDOW: f32 = 0.010;
/// The scope runs the synth at a multiple of the audio rate so its lines are smooth.
const SCOPE_OVERSAMPLE: u32 = 4;
/// Frequency range of the filter graph, in Hz.
const GRAPH_FREQ: (f32, f32) = (20.0, 20000.0);
/// Loudness range of the filter graph, in dB.
const GRAPH_DB: (f32, f32) = (-36.0, 27.0);

const BG: Color32 = Color32::from_rgb(15, 17, 21);
const PANEL: Color32 = Color32::from_rgb(23, 26, 33);
const RAISED: Color32 = Color32::from_rgb(36, 41, 52);
const TRACK: Color32 = Color32::from_rgb(44, 49, 60);
const GHOST: Color32 = Color32::from_rgb(70, 78, 95);
const DIM: Color32 = Color32::from_rgb(120, 128, 145);
const TEXT: Color32 = Color32::from_rgb(225, 229, 238);
const ACCENT: Color32 = Color32::from_rgb(94, 230, 208);

fn stroke(width: f32, color: Color32) -> Stroke {
    Stroke::new(width, color)
}

fn blend(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(mix(from.r(), to.r()), mix(from.g(), to.g()), mix(from.b(), to.b()))
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Wave,
    Sample,
    Filter,
    Lfo,
}

struct Pulsar {
    sink: Option<Sink>,
    _stream: Option<OutputStream>,
    params: Arc<Params>,
    tab: Tab,
    sample: Option<Arc<SampleData>>,
    sample_name: String,
    /// Lowest and highest value in each slice of the sample, for drawing it
    sample_peaks: Vec<(f32, f32)>,
    sample_error: Option<String>,
    /// Where the scope thinks playback is within the cycle. It follows the
    /// audio thread's reports but moves smoothly between them.
    scope_phase: f32,
}

impl Pulsar {
    fn new() -> Self {
        let mut app = Self {
            sink: None,
            _stream: None,
            params: Arc::new(Params::default()),
            tab: Tab::Wave,
            sample: None,
            sample_name: String::new(),
            sample_peaks: Vec::new(),
            sample_error: None,
            scope_phase: 0.0,
        };
        // The built-in sample is ready in the Sample tab from the start; the
        // synth still opens on the waves until Source is switched over.
        if let Ok(data) = SampleData::from_bytes(DEFAULT_SAMPLE) {
            app.set_sample(data, DEFAULT_SAMPLE_NAME.to_string());
        }
        app
    }

    /// Loads a sound file and switches the synth over to playing it.
    fn load_sample(&mut self, path: &Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match SampleData::from_file(path) {
            Ok(data) => {
                self.set_sample(data, name);
                self.params.use_sample.store(true, Ordering::Relaxed);
                self.tab = Tab::Sample;
            }
            Err(error) => self.sample_error = Some(format!("Couldn't load {name}: {error}")),
        }
    }

    /// Makes `data` the current sample, without changing what is playing.
    fn set_sample(&mut self, data: SampleData, name: String) {
        let slices = 480;
        let per_slice = (data.samples.len() / slices).max(1);
        self.sample_peaks = data
            .samples
            .chunks(per_slice)
            .map(|chunk| {
                let low = chunk.iter().copied().fold(f32::MAX, f32::min);
                let high = chunk.iter().copied().fold(f32::MIN, f32::max);
                (low, high)
            })
            .collect();
        let data = Arc::new(data);
        self.params.set_sample(Some(data.clone()));
        self.sample = Some(data);
        self.sample_name = name;
        self.sample_error = None;
    }

    fn choose_sample(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "mp3", "flac", "ogg"])
            .pick_file();
        if let Some(path) = picked {
            self.load_sample(&path);
        }
    }

    fn sampling(&self) -> bool {
        self.sample.is_some() && self.params.use_sample.load(Ordering::Relaxed)
    }

    fn is_playing(&self) -> bool {
        self.sink.is_some()
    }

    fn start_sound(&mut self) {
        if self.sink.is_some() {
            return;
        }

        let (stream, stream_handle) = OutputStream::try_default().unwrap();
        let sink = Sink::try_new(&stream_handle).unwrap();
        sink.append(Oscillator::new(self.params.clone(), SAMPLE_RATE));
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

    fn toggle_sound(&mut self) {
        if self.is_playing() {
            self.stop_sound();
        } else {
            self.start_sound();
        }
    }

    fn wave_tab(&mut self, ui: &mut Ui, shape_now: f32) {
        let sampling = self.sampling();
        let params = &self.params;
        let shape_before = params.shape.load(Ordering::Relaxed);

        // One button per wave. They light up by how much of each wave is in
        // the sound right now, so the LFO can be seen sweeping across them.
        strip(ui, WaveType::ALL.len(), |ui, i| {
            let wave = WaveType::ALL[i];
            // Nothing is lit while the sample is playing instead.
            let weight = if sampling { 0.0 } else { (1.0 - (shape_now - i as f32).abs()).max(0.0) };
            let icon = |t: f32| match wave {
                WaveType::Noise => noise((t * 40.0) as u32 * 31 + 5),
                // Stop just short of a full cycle so the saw doesn't wrap at the edge.
                _ => wave_value(&wave, t * 0.999),
            };
            if shape_button(ui, wave.name(), weight, icon).clicked() {
                params.shape.store(i as f32, Ordering::Relaxed);
                params.use_sample.store(false, Ordering::Relaxed);
            }
        });

        knob_row(ui, vec![
            (&params.frequency, Knob::new("Frequency", (20.0, 10000.0), 440.0).logarithmic().format(hertz)),
            (&params.shape, Knob::new("Wave", (0.0, MAX_SHAPE), 0.0).format(wave_blend)),
            (&params.silence, Knob::new("Silence", (0.0, 1.0), 1.0)),
        ]);

        // Picking a wave with the knob also switches back from the sample.
        if params.shape.load(Ordering::Relaxed) != shape_before {
            params.use_sample.store(false, Ordering::Relaxed);
        }
    }

    fn sample_tab(&mut self, ui: &mut Ui, playhead: Option<f32>) {
        let sampling = self.sampling();
        let loaded = self.sample.as_ref().map(|s| (self.sample_name.as_str(), s.duration()));
        let mut choose = sample_strip(ui, loaded, &self.sample_peaks, sampling, playhead, self.sample_error.as_deref()).clicked();

        let params = &self.params;
        let gated = params.gated.load(Ordering::Relaxed);
        // The Gates knob only appears while the gate is on.
        let columns = if gated { 5 } else { 4 };
        ui.columns(columns, |cols| {
            cols[0].vertical_centered(|ui| {
                let knob = Knob::new("Speed", (0.1, 500.0), 1.0).logarithmic().format(speed);
                bound_knob(ui, &params.speed, knob);
            });
            cols[1].vertical_centered(|ui| {
                bound_knob(ui, &params.silence, Knob::new("Silence", (0.0, 1.0), 1.0));
            });
            cols[2].vertical_centered(|ui| {
                // What Silence does: squeeze the sample and leave a quiet tail,
                // or chop it with regular quiet gaps.
                let text = if gated { "Gate" } else { "Tail" };
                if switch(ui, gated, text, "Silence as").clicked() {
                    params.gated.store(!gated, Ordering::Relaxed);
                }
            });
            if gated {
                cols[3].vertical_centered(|ui| {
                    let knob = Knob::new("Gates", (1.0, 256.0), 16.0).logarithmic().format(|v| format!("{:.0}", v.round()));
                    bound_knob(ui, &params.gates, knob);
                });
            }
            cols[columns - 1].vertical_centered(|ui| {
                let text = if sampling { "Sample" } else { "Waves" };
                if switch(ui, sampling, text, "Source").clicked() {
                    if loaded.is_some() {
                        params.use_sample.store(!sampling, Ordering::Relaxed);
                    } else {
                        choose = true;
                    }
                }
            });
        });

        if choose {
            self.choose_sample();
        }
    }

    fn filter_tab(&mut self, ui: &mut Ui, settings: &Settings, lfo: f32) {
        let params = &self.params;
        filter_graph(ui, params, settings, lfo);
        knob_row(ui, vec![
            (&params.cutoff, Knob::new("Cutoff", (20.0, 18000.0), 18000.0).logarithmic().format(hertz)),
            (&params.resonance, Knob::new("Resonance", (0.0, 1.0), 0.1).percent()),
            (&params.filter_morph, Knob::new("Type", (0.0, 1.0), 0.0).format(filter_type)),
        ]);
    }

    fn lfo_tab(&mut self, ui: &mut Ui, settings: &Settings) {
        let params = &self.params;
        let current = params.lfo_shape();

        strip(ui, LfoShape::ALL.len(), |ui, i| {
            let shape = LfoShape::ALL[i];
            let icon = |t: f32| match shape {
                LfoShape::Random => [0.3, -0.7, 0.8, -0.2, 0.5][((t * 5.0) as usize).min(4)],
                _ => lfo_value(&shape, t * 0.999),
            };
            let weight = if shape == current { 1.0 } else { 0.0 };
            if shape_button(ui, shape.name(), weight, icon).clicked() {
                params.set_lfo_shape(shape);
            }
        });

        let sampling = settings.use_sample && self.sample.is_some();
        ui.columns(3, |cols| {
            cols[0].vertical_centered(|ui| {
                let knob = Knob::new("Rate", (0.05, 20.0), 1.0).logarithmic().format(|v| format!("{v:.2} Hz"));
                bound_knob(ui, &params.lfo_rate, knob);
            });
            cols[1].vertical_centered(|ui| {
                bound_knob(ui, &params.lfo_depth, Knob::new("Depth", (0.0, 1.0), 0.5).percent());
            });
            cols[2].vertical_centered(|ui| lfo_targets(ui, params, settings, sampling));
        });
    }
}

impl App for Pulsar {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        // Space plays and stops, unless a number is being typed into a knob.
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Space)) {
            self.toggle_sound();
        }
        // A sound file dropped anywhere on the window gets loaded.
        let dropped = ctx.input(|i| i.raw.dropped_files.first().and_then(|file| file.path.clone()));
        if let Some(path) = dropped {
            self.load_sample(&path);
        }

        let frame = egui::Frame::none().fill(BG).inner_margin(20.0);
        CentralPanel::default().frame(frame).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 14.0);

            let settings = self.params.load();
            let playing = self.is_playing();
            // Where the Wave position really is: the audio thread reports it
            // while playing, since the LFO moves it.
            let shape_now = if playing {
                self.params.shape_now.load(Ordering::Relaxed)
            } else {
                settings.shape
            };

            let time = ui.input(|i| i.time);
            let sample = if settings.use_sample { self.sample.clone() } else { None };
            let lfo = if playing { self.params.lfo_now.load(Ordering::Relaxed) } else { 0.0 };
            let moved = settings.modulated(lfo);

            // Follow the audio thread's position. It reports in bursts, so move
            // at the expected speed and lean gently toward each report.
            let live_phase = if playing {
                let cycles_per_second = match &sample {
                    Some(sample) => moved.speed / sample.duration(),
                    None => moved.frequency,
                };
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                let reported = self.params.phase_now.load(Ordering::Relaxed);
                let mut phase = self.scope_phase + dt * cycles_per_second;
                let mut gap = reported - phase;
                gap -= gap.round();
                phase += if gap.abs() > 0.2 { gap } else { gap * 0.1 };
                self.scope_phase = phase.rem_euclid(1.0);
                Some(self.scope_phase)
            } else {
                None
            };
            // Where in the sample that is; nothing during the silent part of the cycle.
            let playhead = live_phase
                .filter(|_| sample.is_some())
                // Gated, the playhead keeps travelling through the quiet slots.
            .and_then(|phase| match moved.gated {
                true => Some(phase),
                false => sample_position(phase, moved.silence, false, moved.gates),
            });

            scope(ui, &settings, sample, lfo, live_phase, time);
            if playing {
                ctx.request_repaint();
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, name) in [(Tab::Wave, "Wave"), (Tab::Sample, "Sample"), (Tab::Filter, "Filter"), (Tab::Lfo, "LFO")] {
                    if tab_button(ui, name, self.tab == tab).clicked() {
                        self.tab = tab;
                    }
                }
            });

            match self.tab {
                Tab::Wave => self.wave_tab(ui, shape_now),
                Tab::Sample => self.sample_tab(ui, playhead),
                Tab::Filter => self.filter_tab(ui, &settings, lfo),
                Tab::Lfo => self.lfo_tab(ui, &settings),
            }

            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, TRACK);

            // Always in reach, whichever tab is open
            ui.columns(2, |cols| {
                cols[0].vertical_centered(|ui| {
                    if play_button(ui, playing).clicked() {
                        self.toggle_sound();
                    }
                });
                cols[1].vertical_centered(|ui| {
                    let knob = Knob::new("Volume", (0.0, 1.0), 0.35).percent();
                    bound_knob(ui, &self.params.volume, knob);
                });
            });
        });
    }
}

// ---------------------------------------------------------------------------
// How knob values are written out
// ---------------------------------------------------------------------------

fn hertz(v: f32) -> String {
    if v >= 1000.0 {
        format!("{:.1} kHz", v / 1000.0)
    } else {
        format!("{v:.0} Hz")
    }
}

fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn speed(v: f32) -> String {
    if v >= 100.0 {
        format!("× {v:.0}")
    } else if v >= 10.0 {
        format!("× {v:.1}")
    } else {
        format!("× {v:.2}")
    }
}

fn wave_blend(v: f32) -> String {
    let nearest = v.round();
    if (v - nearest).abs() < 0.03 {
        return WaveType::ALL[nearest as usize].name().to_string();
    }
    let lower = v.floor() as usize;
    format!("{} + {}", WaveType::ALL[lower].name(), WaveType::ALL[lower + 1].name())
}

fn filter_type(v: f32) -> String {
    let name = if v < 0.03 {
        "Lowpass"
    } else if (v - 0.5).abs() < 0.03 {
        "Bandpass"
    } else if v > 0.97 {
        "Highpass"
    } else if v < 0.5 {
        "Low + Band"
    } else {
        "Band + High"
    };
    name.to_string()
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

/// A rotary knob. Drag up/down (or left/right) to turn it, hold Shift for fine
/// control, scroll to nudge, click to type a value, double-click to reset.
struct Knob {
    label: &'static str,
    range: (f32, f32),
    default: f32,
    logarithmic: bool,
    format: fn(f32) -> String,
    /// What a typed number is divided by to get the value (100 for percentages)
    typed_scale: f32,
}

impl Knob {
    fn new(label: &'static str, range: (f32, f32), default: f32) -> Self {
        Self {
            label,
            range,
            default,
            logarithmic: false,
            format: |v| format!("{v:.2}"),
            typed_scale: 1.0,
        }
    }

    fn logarithmic(mut self) -> Self {
        self.logarithmic = true;
        self
    }

    fn format(mut self, format: fn(f32) -> String) -> Self {
        self.format = format;
        self
    }

    /// Shown, and typed, as 0 to 100%
    fn percent(mut self) -> Self {
        self.format = percent;
        self.typed_scale = 100.0;
        self
    }

    /// Value -> position on the dial, 0.0..=1.0
    fn to_dial(&self, value: f32) -> f32 {
        let (min, max) = self.range;
        let t = if self.logarithmic {
            (value / min).ln() / (max / min).ln()
        } else {
            (value - min) / (max - min)
        };
        t.clamp(0.0, 1.0)
    }

    fn from_dial(&self, t: f32) -> f32 {
        let (min, max) = self.range;
        if self.logarithmic {
            min * (max / min).powf(t)
        } else {
            min + (max - min) * t
        }
    }

    /// Reads a typed value such as "880", "1.5k", "40%" or "x2", kept within range.
    fn parse(&self, text: &str) -> Option<f32> {
        let text = text.trim().to_lowercase();
        let digits: String = text.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
        let mut number: f32 = digits.parse().ok()?;
        if text.contains('k') {
            number *= 1000.0;
        }
        Some((number / self.typed_scale).clamp(self.range.0, self.range.1))
    }

    fn show(self, ui: &mut Ui, value: &mut f32) -> Response {
        let (rect, mut response) = ui.allocate_exact_size(vec2(ui.available_width().min(110.0), 122.0), Sense::click_and_drag());
        let mut dial = self.to_dial(*value);
        let before = dial;

        // The text being typed, if this knob's value is being edited.
        let edit_id = response.id.with("typed value");
        let mut typed: Option<String> = ui.data(|data| data.get_temp(edit_id));
        let was_typing = typed.is_some();

        if response.double_clicked() {
            dial = self.to_dial(self.default);
            typed = None;
        } else if response.clicked() && typed.is_none() {
            typed = Some(String::new());
            ui.memory_mut(|memory| memory.request_focus(edit_id));
        } else if response.dragged() {
            let delta = response.drag_delta();
            let fine = ui.input(|i| i.modifiers.shift);
            let pixels_per_turn = if fine { 1200.0 } else { 180.0 };
            dial += (delta.x - delta.y) / pixels_per_turn;
        } else if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            dial += scroll / 1500.0;
        }
        dial = dial.clamp(0.0, 1.0);

        if dial != before {
            *value = self.from_dial(dial);
            response.mark_changed();
        }

        // The text box appears the frame after the click that asked for it, so
        // that same click doesn't count as clicking away from it.
        let value_center = pos2(rect.center().x, rect.top() + 94.0);
        if let (true, Some(text)) = (was_typing, typed.as_mut()) {
            let field = Rect::from_center_size(value_center, vec2(86.0, 22.0));
            let edit = egui::TextEdit::singleline(text)
                .id(edit_id)
                .hint_text((self.format)(*value))
                .horizontal_align(egui::Align::Center)
                .font(FontId::proportional(15.0));
            if ui.put(field, edit).lost_focus() {
                // Enter or clicking away accepts; Escape cancels.
                if !ui.input(|i| i.key_pressed(Key::Escape)) {
                    if let Some(new_value) = self.parse(text) {
                        *value = new_value;
                        dial = self.to_dial(new_value);
                        response.mark_changed();
                    }
                }
                typed = None;
            }
        }
        match &typed {
            Some(text) => {
                ui.data_mut(|data| data.insert_temp(edit_id, text.clone()));
                if !was_typing {
                    ui.ctx().request_repaint();
                }
            }
            None => ui.data_mut(|data| data.remove::<String>(edit_id)),
        }

        // The dial sweeps 270 degrees, with the gap at the bottom.
        let active = response.dragged() || response.hovered();
        let center = pos2(rect.center().x, rect.top() + 46.0);
        let radius = 34.0;
        let angle = |t: f32| PI * 0.75 + t * PI * 1.5;
        let arc = |from: f32, to: f32| -> Vec<Pos2> {
            let steps = (((to - from) * 60.0).ceil() as usize).max(1);
            (0..=steps)
                .map(|i| {
                    let a = angle(from + (to - from) * i as f32 / steps as f32);
                    center + radius * vec2(a.cos(), a.sin())
                })
                .collect()
        };

        let painter = ui.painter();
        painter.add(Shape::line(arc(0.0, 1.0), stroke(5.0, TRACK)));
        if dial > 0.0 {
            painter.add(Shape::line(arc(0.0, dial), stroke(5.0, ACCENT)));
        }
        painter.circle_filled(center, radius - 9.0, if active { RAISED } else { PANEL });
        let dir = vec2(angle(dial).cos(), angle(dial).sin());
        painter.line_segment(
            [center + dir * 10.0, center + dir * (radius - 12.0)],
            stroke(3.0, if active { ACCENT } else { TEXT }),
        );

        if !(was_typing && typed.is_some()) {
            painter.text(
                value_center,
                Align2::CENTER_CENTER,
                (self.format)(*value),
                FontId::proportional(15.0),
                TEXT,
            );
        }
        painter.text(
            pos2(center.x, rect.top() + 112.0),
            Align2::CENTER_CENTER,
            self.label,
            FontId::proportional(12.0),
            DIM,
        );

        response
            .on_hover_cursor(CursorIcon::ResizeVertical)
            .on_hover_text("Drag to turn · click to type a value · Shift for fine · double-click to reset")
    }
}

/// The switches for which controls the LFO moves.
fn lfo_targets(ui: &mut Ui, params: &Params, settings: &Settings, sampling: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(146.0, 122.0), Sense::hover());
    let names = [
        (LfoTarget::Wave, "Wave"),
        (LfoTarget::Pitch, if sampling { "Speed" } else { "Pitch" }),
        (LfoTarget::Silence, "Silence"),
        (LfoTarget::Cutoff, "Cutoff"),
        (LfoTarget::Resonance, "Resonance"),
        (LfoTarget::FilterType, "Type"),
    ];

    for (i, (target, name)) in names.into_iter().enumerate() {
        let corner = rect.left_top() + vec2((i % 2) as f32 * 76.0, 8.0 + (i / 2) as f32 * 30.0);
        let chip = Rect::from_min_size(corner, vec2(70.0, 24.0));
        let response = ui
            .interact(chip, ui.id().with(("lfo target", i)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            params.toggle_lfo_target(target);
        }

        // A click flips it, so show the opposite of what was read this frame.
        let on = settings.lfo_moves(target) != response.clicked();
        let painter = ui.painter();
        painter.rect_filled(chip, 12.0, if on || response.hovered() { RAISED } else { PANEL });
        if on {
            painter.rect_stroke(chip.shrink(0.5), 12.0, stroke(1.0, ACCENT));
        }
        painter.text(
            chip.center(),
            Align2::CENTER_CENTER,
            name,
            FontId::proportional(11.0),
            if on { TEXT } else { DIM },
        );
    }

    ui.painter().text(
        pos2(rect.center().x, rect.top() + 112.0),
        Align2::CENTER_CENTER,
        "LFO moves",
        FontId::proportional(12.0),
        DIM,
    );
}

/// A knob wired straight to one of the live synth controls.
fn bound_knob(ui: &mut Ui, param: &AtomicF32, knob: Knob) {
    let mut value = param.load(Ordering::Relaxed);
    if knob.show(ui, &mut value).changed() {
        param.store(value, Ordering::Relaxed);
    }
}

/// A row of knobs spread evenly across the window.
fn knob_row(ui: &mut Ui, knobs: Vec<(&AtomicF32, Knob)>) {
    ui.columns(knobs.len(), |cols| {
        for (col, (param, knob)) in cols.iter_mut().zip(knobs) {
            col.vertical_centered(|ui| bound_knob(ui, param, knob));
        }
    });
}

/// A row of `count` equal-width cells with tight gaps, for the shape buttons.
fn strip(ui: &mut Ui, count: usize, mut cell: impl FnMut(&mut Ui, usize)) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.columns(count, |cols| {
            for (i, col) in cols.iter_mut().enumerate() {
                cell(col, i);
            }
        });
    });
}

/// A button showing one cycle of a wave. `weight` (0.0..=1.0) is how lit it is.
fn shape_button(ui: &mut Ui, name: &str, weight: f32, icon: impl Fn(f32) -> f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
    let painter = ui.painter_at(rect);

    let base = if response.hovered() { RAISED } else { PANEL };
    painter.rect_filled(rect, 8.0, blend(base, RAISED, weight));
    if weight > 0.01 {
        let edge = blend(PANEL, ACCENT, weight);
        painter.rect_stroke(rect.shrink(0.5), 8.0, stroke(1.0, edge));
    }

    let width = (rect.width() - 16.0).min(44.0);
    let area = Rect::from_center_size(rect.center() - vec2(0.0, 9.0), vec2(width, 22.0));
    let steps = 44;
    let line: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            pos2(area.left() + t * area.width(), area.center().y - icon(t) * area.height() / 2.0)
        })
        .collect();
    painter.add(Shape::line(line, stroke(1.5, blend(DIM, ACCENT, weight))));

    painter.text(
        pos2(rect.center().x, rect.bottom() - 13.0),
        Align2::CENTER_CENTER,
        name,
        FontId::proportional(11.0),
        blend(DIM, TEXT, weight),
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

fn tab_button(ui: &mut Ui, name: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(72.0, 28.0), Sense::click());
    let painter = ui.painter();
    let color = if selected || response.hovered() { TEXT } else { DIM };
    painter.text(rect.center(), Align2::CENTER_CENTER, name, FontId::proportional(14.0), color);
    if selected {
        let underline = Rect::from_min_max(
            pos2(rect.left() + 14.0, rect.bottom() - 2.0),
            pos2(rect.right() - 14.0, rect.bottom()),
        );
        painter.rect_filled(underline, 1.0, ACCENT);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// The loaded sample drawn end to end. Click it to load a different file.
fn sample_strip(
    ui: &mut Ui,
    loaded: Option<(&str, f32)>,
    peaks: &[(f32, f32)],
    active: bool,
    playhead: Option<f32>,
    error: Option<&str>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, if response.hovered() { RAISED } else { PANEL });
    if active {
        painter.rect_stroke(rect.shrink(0.5), 8.0, stroke(1.0, ACCENT));
    }

    let ink = if active { ACCENT } else { DIM };
    let mid = rect.center().y + 6.0;
    let amp = 20.0;
    for (i, (low, high)) in peaks.iter().enumerate() {
        let x = rect.left() + 8.0 + (rect.width() - 16.0) * i as f32 / peaks.len().max(1) as f32;
        let top = mid - high.clamp(-1.0, 1.0) * amp;
        let bottom = mid - low.clamp(-1.0, 1.0) * amp;
        painter.line_segment([pos2(x, top - 0.5), pos2(x, bottom + 0.5)], stroke(1.0, ink));
    }

    // Where in the sample playback is right now
    if let Some(position) = playhead {
        let x = rect.left() + 8.0 + (rect.width() - 16.0) * position.clamp(0.0, 1.0);
        painter.line_segment([pos2(x, mid - amp - 2.0), pos2(x, mid + amp + 2.0)], stroke(2.0, TEXT));
    }

    let small = FontId::proportional(11.0);
    match (error, loaded) {
        (Some(error), _) => {
            let warning = Color32::from_rgb(240, 130, 120);
            painter.text(rect.left_top() + vec2(10.0, 7.0), Align2::LEFT_TOP, error, small, warning);
        }
        (None, Some((name, seconds))) => {
            let title = format!("{name}  ·  {seconds:.2} s");
            painter.text(rect.left_top() + vec2(10.0, 7.0), Align2::LEFT_TOP, title, small.clone(), TEXT);
            painter.text(rect.right_top() + vec2(-10.0, 7.0), Align2::RIGHT_TOP, "click to replace", small, DIM);
        }
        (None, None) => {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Click to load a sample, or drop a sound file on the window",
                FontId::proportional(13.0),
                DIM,
            );
        }
    }

    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A two-way switch with its current choice written underneath.
fn switch(ui: &mut Ui, on: bool, choice: &str, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width().min(110.0), 122.0), Sense::click());
    let painter = ui.painter();
    let center = pos2(rect.center().x, rect.top() + 46.0);

    let pill = Rect::from_center_size(center, vec2(60.0, 30.0));
    let fill = if on { ACCENT } else if response.hovered() { RAISED } else { TRACK };
    painter.rect_filled(pill, 15.0, fill);
    let handle_x = if on { pill.right() - 15.0 } else { pill.left() + 15.0 };
    painter.circle_filled(pos2(handle_x, center.y), 11.0, if on { BG } else { TEXT });

    painter.text(
        pos2(center.x, rect.top() + 94.0),
        Align2::CENTER_CENTER,
        choice,
        FontId::proportional(15.0),
        TEXT,
    );
    painter.text(
        pos2(center.x, rect.top() + 112.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.0),
        DIM,
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// One round button that plays or stops.
fn play_button(ui: &mut Ui, playing: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(110.0, 122.0), Sense::click());
    let painter = ui.painter();
    let center = pos2(rect.center().x, rect.top() + 46.0);
    let radius = if response.hovered() { 30.0 } else { 28.0 };

    if playing {
        painter.circle_filled(center, radius, ACCENT);
        painter.rect_filled(Rect::from_center_size(center, vec2(18.0, 18.0)), 3.0, BG);
    } else {
        painter.circle_stroke(center, radius, stroke(2.0, ACCENT));
        painter.add(Shape::convex_polygon(
            vec![
                center + vec2(-7.0, -11.0),
                center + vec2(12.0, 0.0),
                center + vec2(-7.0, 11.0),
            ],
            ACCENT,
            Stroke::NONE,
        ));
    }

    painter.text(
        pos2(center.x, rect.top() + 94.0),
        Align2::CENTER_CENTER,
        if playing { "Stop" } else { "Play" },
        FontId::proportional(15.0),
        TEXT,
    );
    painter.text(
        pos2(center.x, rect.top() + 112.0),
        Align2::CENTER_CENTER,
        "Space",
        FontId::proportional(12.0),
        DIM,
    );

    response.on_hover_cursor(CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// Displays
// ---------------------------------------------------------------------------

/// Cheap repeatable noise in -1.0..1.0, for drawing the Noise icon.
fn noise(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Turns a run of values into a line across `rect`. When there are far more
/// values than pixels, each pixel column gets its highest and lowest value so
/// no peaks are skipped.
fn trace(rect: Rect, values: &[f32], y_of: impl Fn(f32) -> f32) -> Vec<Pos2> {
    let columns = rect.width().max(1.0) as usize;
    if values.len() <= columns * 2 {
        let last = (values.len().max(2) - 1) as f32;
        return values
            .iter()
            .enumerate()
            .map(|(i, v)| pos2(rect.left() + rect.width() * i as f32 / last, y_of(*v)))
            .collect();
    }

    let mut points = Vec::with_capacity(columns * 2);
    for column in 0..columns {
        let from = column * values.len() / columns;
        let to = ((column + 1) * values.len() / columns).max(from + 1);
        let slice = &values[from..to];
        let low = slice.iter().copied().fold(f32::MAX, f32::min);
        let high = slice.iter().copied().fold(f32::MIN, f32::max);
        let x = rect.left() + column as f32 + 0.5;
        points.push(pos2(x, y_of(high)));
        points.push(pos2(x, y_of(low)));
    }
    points
}

/// Draws the sound straight from the oscillator or sample (dim) and what is
/// left of it after the silence and the filter (bright). It runs the real
/// synth code for the length of the window, so it matches what is heard.
fn scope(
    ui: &mut Ui,
    settings: &Settings,
    sample: Option<Arc<SampleData>>,
    lfo: f32,
    live_phase: Option<f32>,
    time: f64,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, PANEL);

    let mid = rect.center().y;
    let amp = rect.height() * 0.40;
    let y_of = |v: f32| mid - v.clamp(-1.0, 1.0) * amp;

    painter.line_segment([pos2(rect.left(), mid), pos2(rect.right(), mid)], stroke(1.0, TRACK));

    // With waves, anything above this level gets dropped to silence.
    // (With a sample, Silence cuts in time instead, so there is no such line.)
    let silence_now = settings.modulated(lfo).silence;
    if sample.is_none() && silence_now < 1.0 {
        let y = y_of(silence_now);
        let mut x = rect.left() + 8.0;
        while x < rect.right() - 8.0 {
            painter.line_segment([pos2(x, y), pos2(x + 5.0, y)], stroke(1.0, DIM));
            x += 10.0;
        }
        painter.text(
            pos2(rect.right() - 10.0, y - 3.0),
            Align2::RIGHT_BOTTOM,
            "cut above",
            FontId::proportional(10.0),
            DIM,
        );
    }

    // Waves get a fixed 10 ms window. A sample gets two of its cycles, up to
    // one second, since a cycle can be anything from a blip to several seconds.
    let cycles_per_second = match &sample {
        Some(sample) => settings.speed / sample.duration(),
        None => settings.frequency,
    };
    let window = match &sample {
        Some(_) => (2.0 / cycles_per_second).clamp(0.002, 1.0),
        None => SCOPE_WINDOW,
    };
    // Short windows are drawn at a multiple of the audio rate so they look smooth.
    let rate = if window > 0.02 { SAMPLE_RATE } else { SAMPLE_RATE * SCOPE_OVERSAMPLE };

    let points = (window * rate as f32) as usize;
    // Run for a while before the visible part so the filter has settled.
    let lead_in = (0.040 * rate as f32) as usize;

    let slow_sample = sample.is_some() && cycles_per_second < 8.0;
    let (start_cycles, seed) = match live_phase {
        // A slow sample scrolls past in real time, with "now" at the right edge.
        Some(phase) if slow_sample => (phase - window * cycles_per_second, 1),
        // Anything faster would be a blur, so drift through it slowly instead.
        Some(_) => ((cycles_per_second as f64 * time * 0.002).fract() as f32, (time * 30.0) as u32),
        None => (0.0, 1),
    };

    let source_label = if sample.is_some() { "sample" } else { "wave" };
    let mut voice = Voice::new(rate);
    voice.set_sample(sample);
    voice.snap(settings);
    // Hold the LFO where it is right now, so the picture is of this instant.
    voice.freeze_lfo(lfo);
    voice.seed(seed.wrapping_mul(7919).wrapping_add(12345));
    voice.set_phase(start_cycles - lead_in as f32 * cycles_per_second / rate as f32);
    for _ in 0..lead_in {
        voice.tick(settings);
    }

    let mut source = Vec::with_capacity(points);
    let mut output = Vec::with_capacity(points);
    for _ in 0..points {
        let frame = voice.tick(settings);
        source.push(frame.raw);
        output.push(frame.filtered);
    }
    let output = trace(rect, &output, y_of);

    painter.add(Shape::line(trace(rect, &source, y_of), stroke(1.5, GHOST)));
    painter.add(Shape::line(output.clone(), stroke(6.0, ACCENT.gamma_multiply(0.15))));
    painter.add(Shape::line(output, stroke(2.0, ACCENT)));

    painter.text(
        rect.left_top() + vec2(12.0, 10.0),
        Align2::LEFT_TOP,
        "PULSAR",
        FontId::proportional(12.0),
        DIM,
    );
    let window_label = if window >= 0.9995 {
        format!("{window:.1} s")
    } else {
        format!("{:.0} ms", window * 1000.0)
    };
    painter.text(
        rect.right_top() + vec2(-12.0, 10.0),
        Align2::RIGHT_TOP,
        window_label,
        FontId::proportional(11.0),
        DIM,
    );
    let legend = painter.text(
        rect.left_bottom() + vec2(12.0, -9.0),
        Align2::LEFT_BOTTOM,
        source_label,
        FontId::proportional(10.0),
        DIM,
    );
    painter.text(
        pos2(legend.right() + 10.0, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        "output",
        FontId::proportional(10.0),
        ACCENT,
    );
}

/// The filter's curve: which frequencies get through, low on the left to high
/// on the right. Drag on it to move the cutoff (sideways) and resonance (up/down).
fn filter_graph(ui: &mut Ui, params: &Params, settings: &Settings, lfo: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click_and_drag());
    let (f_min, f_max) = GRAPH_FREQ;
    let (db_min, db_max) = GRAPH_DB;

    if response.dragged() || response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let tx = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let ty = ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            let cutoff = (f_min * (f_max / f_min).powf(tx)).clamp(20.0, 18000.0);
            params.cutoff.store(cutoff, Ordering::Relaxed);
            params.resonance.store(1.0 - ty, Ordering::Relaxed);
        }
    }
    // Re-read so the curve follows the pointer on this very frame, then apply
    // the LFO so the curve shows the filter as it is right now.
    let shown = Settings {
        cutoff: params.cutoff.load(Ordering::Relaxed),
        resonance: params.resonance.load(Ordering::Relaxed),
        ..*settings
    }
    .modulated(lfo);
    let (cutoff, resonance) = (shown.cutoff, shown.resonance);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, if response.hovered() { RAISED } else { PANEL });

    let x_of = |freq: f32| rect.left() + rect.width() * (freq / f_min).ln() / (f_max / f_min).ln();
    let y_of = |db: f32| rect.top() + rect.height() * (db_max - db.clamp(db_min, db_max)) / (db_max - db_min);

    // Unchanged-loudness line and the cutoff marker
    painter.line_segment([pos2(rect.left(), y_of(0.0)), pos2(rect.right(), y_of(0.0))], stroke(1.0, TRACK));
    let cutoff_x = x_of(cutoff);
    painter.line_segment([pos2(cutoff_x, rect.top()), pos2(cutoff_x, rect.bottom())], stroke(1.0, TRACK));

    let steps = (rect.width() / 2.0) as usize;
    let curve: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let freq = f_min * (f_max / f_min).powf(t);
            let gain = filter_response(freq, cutoff, resonance, shown.filter_morph);
            pos2(rect.left() + t * rect.width(), y_of(20.0 * gain.max(1e-6).log10()))
        })
        .collect();
    painter.add(Shape::line(curve.clone(), stroke(6.0, ACCENT.gamma_multiply(0.15))));
    painter.add(Shape::line(curve, stroke(2.0, ACCENT)));

    painter.text(rect.left_bottom() + vec2(10.0, -6.0), Align2::LEFT_BOTTOM, "low", FontId::proportional(10.0), DIM);
    painter.text(rect.right_bottom() + vec2(-10.0, -6.0), Align2::RIGHT_BOTTOM, "high", FontId::proportional(10.0), DIM);

    response
        .on_hover_cursor(CursorIcon::Crosshair)
        .on_hover_text("Drag sideways for cutoff, up and down for resonance");
}

fn main() -> eframe::Result {
    let app = Pulsar::new();
    let win_option = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 600.0])
            .with_min_inner_size([480.0, 600.0]),
        ..Default::default()
    };
    run_native(
        "Pulsar",
        win_option,
        Box::new(|cc| {
            // Dark everywhere, including tooltips and the type-a-value boxes.
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(app))
        }),
    )
}
