use atomic_float::AtomicF32;
use eframe::{egui::CentralPanel, run_native, App, NativeOptions};
use egui::{
    pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Pos2, Rect, Response, Sense, Shape,
    Stroke, Ui,
};
use rodio::{OutputStream, Sink};
use std::f32::consts::PI;
use std::sync::{atomic::Ordering, Arc};
use synth::{
    filter_response, lfo_value, wave_value, LfoShape, Oscillator, Params, Settings, Voice,
    WaveType, MAX_SHAPE,
};

const SAMPLE_RATE: u32 = 44100;
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
    Filter,
    Lfo,
}

struct Pulsar {
    sink: Option<Sink>,
    _stream: Option<OutputStream>,
    params: Arc<Params>,
    tab: Tab,
}

impl Pulsar {
    fn new() -> Self {
        Self {
            sink: None,
            _stream: None,
            params: Arc::new(Params::default()),
            tab: Tab::Wave,
        }
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
        let params = &self.params;

        // One button per wave. They light up by how much of each wave is in
        // the sound right now, so the LFO can be seen sweeping across them.
        strip(ui, WaveType::ALL.len(), |ui, i| {
            let wave = WaveType::ALL[i];
            let weight = (1.0 - (shape_now - i as f32).abs()).max(0.0);
            let icon = |t: f32| match wave {
                WaveType::Noise => noise((t * 40.0) as u32 * 31 + 5),
                // Stop just short of a full cycle so the saw doesn't wrap at the edge.
                _ => wave_value(&wave, t * 0.999),
            };
            if shape_button(ui, wave.name(), weight, icon).clicked() {
                params.shape.store(i as f32, Ordering::Relaxed);
            }
        });

        knob_row(ui, vec![
            (&params.frequency, Knob::new("Frequency", (20.0, 2000.0), 440.0).logarithmic().format(hertz)),
            (&params.shape, Knob::new("Wave", (0.0, MAX_SHAPE), 0.0).format(wave_blend)),
            (&params.silence, Knob::new("Silence", (0.0, 1.0), 1.0)),
        ]);
    }

    fn filter_tab(&mut self, ui: &mut Ui, settings: &Settings) {
        let params = &self.params;
        filter_graph(ui, params, settings);
        knob_row(ui, vec![
            (&params.cutoff, Knob::new("Cutoff", (20.0, 18000.0), 18000.0).logarithmic().format(hertz)),
            (&params.resonance, Knob::new("Resonance", (0.0, 1.0), 0.1).format(percent)),
            (&params.filter_morph, Knob::new("Type", (0.0, 1.0), 0.0).format(filter_type)),
        ]);
    }

    fn lfo_tab(&mut self, ui: &mut Ui) {
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

        knob_row(ui, vec![
            (&params.lfo_rate, Knob::new("Rate", (0.05, 20.0), 1.0).logarithmic().format(|v| format!("{v:.2} Hz"))),
            (&params.lfo_depth, Knob::new("Depth", (0.0, 4.0), 0.0).format(lfo_depth)),
        ]);
    }
}

impl App for Pulsar {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            self.toggle_sound();
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
            scope(ui, &settings, shape_now, playing, time);
            if playing {
                ctx.request_repaint();
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, name) in [(Tab::Wave, "Wave"), (Tab::Filter, "Filter"), (Tab::Lfo, "LFO")] {
                    if tab_button(ui, name, self.tab == tab).clicked() {
                        self.tab = tab;
                    }
                }
            });

            match self.tab {
                Tab::Wave => self.wave_tab(ui, shape_now),
                Tab::Filter => self.filter_tab(ui, &settings),
                Tab::Lfo => self.lfo_tab(ui),
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
                    let knob = Knob::new("Volume", (0.0, 1.0), 0.35).format(percent);
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

fn lfo_depth(v: f32) -> String {
    if v < 0.02 {
        "Off".to_string()
    } else {
        format!("± {v:.1} waves")
    }
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

/// A rotary knob. Drag up/down (or left/right) to turn it, hold Shift for fine
/// control, scroll to nudge, double-click to reset.
struct Knob {
    label: &'static str,
    range: (f32, f32),
    default: f32,
    logarithmic: bool,
    format: fn(f32) -> String,
}

impl Knob {
    fn new(label: &'static str, range: (f32, f32), default: f32) -> Self {
        Self {
            label,
            range,
            default,
            logarithmic: false,
            format: |v| format!("{v:.2}"),
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

    fn show(self, ui: &mut Ui, value: &mut f32) -> Response {
        let (rect, mut response) = ui.allocate_exact_size(vec2(110.0, 122.0), Sense::click_and_drag());
        let mut dial = self.to_dial(*value);
        let before = dial;

        if response.double_clicked() {
            dial = self.to_dial(self.default);
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

        painter.text(
            pos2(center.x, rect.top() + 94.0),
            Align2::CENTER_CENTER,
            (self.format)(*value),
            FontId::proportional(15.0),
            TEXT,
        );
        painter.text(
            pos2(center.x, rect.top() + 112.0),
            Align2::CENTER_CENTER,
            self.label,
            FontId::proportional(12.0),
            DIM,
        );

        response
            .on_hover_cursor(CursorIcon::ResizeVertical)
            .on_hover_text("Drag to turn · Shift for fine · double-click to reset")
    }
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

/// Draws the wave straight from the oscillator (dim) and what is left of it
/// after the silence distortion and the filter (bright). It runs the real
/// synth code for the length of the window, so it matches what is heard.
fn scope(ui: &mut Ui, settings: &Settings, shape_now: f32, playing: bool, time: f64) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, PANEL);

    let mid = rect.center().y;
    let amp = rect.height() * 0.40;
    let y_of = |v: f32| mid - v.clamp(-1.0, 1.0) * amp;

    painter.line_segment([pos2(rect.left(), mid), pos2(rect.right(), mid)], stroke(1.0, TRACK));

    // Anything in the wave above this level gets dropped to silence.
    if settings.silence < 1.0 {
        let y = y_of(settings.silence);
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

    // Freeze the LFO at its current position, and leave volume out so a quiet
    // setting doesn't flatten the picture.
    let frozen = Settings { shape: shape_now, lfo_depth: 0.0, ..*settings };
    let rate = SAMPLE_RATE * SCOPE_OVERSAMPLE;
    let points = (SCOPE_WINDOW * rate as f32) as usize;
    // Run for a while before the visible part so the filter has settled.
    let lead_in = (0.040 * rate as f32) as usize;

    // While playing, drift slowly through the wave so it reads as live.
    let (start_cycles, seed) = if playing {
        ((settings.frequency as f64 * time * 0.002).fract() as f32, (time * 30.0) as u32)
    } else {
        (0.0, 1)
    };

    let mut voice = Voice::new(rate);
    voice.snap(&frozen);
    voice.seed(seed.wrapping_mul(7919).wrapping_add(12345));
    voice.set_phase(start_cycles - lead_in as f32 * settings.frequency / rate as f32);
    for _ in 0..lead_in {
        voice.tick(&frozen);
    }

    let mut wave = Vec::with_capacity(points);
    let mut output = Vec::with_capacity(points);
    for i in 0..points {
        let frame = voice.tick(&frozen);
        let x = rect.left() + rect.width() * i as f32 / (points - 1) as f32;
        wave.push(pos2(x, y_of(frame.raw)));
        output.push(pos2(x, y_of(frame.filtered)));
    }

    painter.add(Shape::line(wave, stroke(1.5, GHOST)));
    painter.add(Shape::line(output.clone(), stroke(6.0, ACCENT.gamma_multiply(0.15))));
    painter.add(Shape::line(output, stroke(2.0, ACCENT)));

    painter.text(
        rect.left_top() + vec2(12.0, 10.0),
        Align2::LEFT_TOP,
        "PULSAR",
        FontId::proportional(12.0),
        DIM,
    );
    painter.text(
        rect.right_top() + vec2(-12.0, 10.0),
        Align2::RIGHT_TOP,
        format!("{:.0} ms", SCOPE_WINDOW * 1000.0),
        FontId::proportional(11.0),
        DIM,
    );
    let legend = painter.text(
        rect.left_bottom() + vec2(12.0, -9.0),
        Align2::LEFT_BOTTOM,
        "wave",
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
fn filter_graph(ui: &mut Ui, params: &Params, settings: &Settings) {
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
    // Re-read so the curve follows the pointer on this very frame.
    let cutoff = params.cutoff.load(Ordering::Relaxed);
    let resonance = params.resonance.load(Ordering::Relaxed);

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
            let gain = filter_response(freq, cutoff, resonance, settings.filter_morph);
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
    run_native("Pulsar", win_option, Box::new(|_cc| Ok(Box::new(app))))
}
