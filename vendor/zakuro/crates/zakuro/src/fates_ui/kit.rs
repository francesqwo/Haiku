//! the Fates UI kit: the game's own window art and bitmap font, decoded
//! from its UI .bch files into assets/, drawn through egui's painter as
//! textured meshes. Everything is laid out in the 3DS's own pixels ("units")
//! and scaled to the window, so it reads at the size the game's own menus do
//! at any resolution.
//!
//! The art is upscaled once on load by a whole factor near the on-screen
//! scale, with premultiplied Catmull-Rom filtering, and then drawn with
//! linear sampling. That keeps the painted textures smooth and the font's
//! edges clean at 4K instead of blurring a 3DS-sized texel over nine pixels.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Color32, ColorImage, Mesh, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;

/// the game's text colours, sampled from its own screens.
pub const TEXT_DARK: Color32 = Color32::from_rgb(68, 8, 0);
pub const TEXT_CREAM: Color32 = Color32::from_rgb(253, 236, 185);
pub const TEXT_GOLD: Color32 = Color32::from_rgb(255, 215, 130);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(132, 104, 84);

/// where the original art is: <FATES_ROOT or the executable's folder>/assets.
pub fn assets_dir() -> PathBuf {
    let root = std::env::var_os("FATES_ROOT").map(PathBuf::from).or_else(|| {
        std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_owned))
    });
    let dir = root.unwrap_or_default().join("assets");
    if dir.join("ui").is_dir() {
        return dir;
    }
    // a development run from the source tree
    PathBuf::from("assets")
}

/// a source rectangle in one of the game's sheets.
#[derive(Clone, Copy)]
struct Src {
    sheet: &'static str,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

const fn src(sheet: &'static str, x: u32, y: u32, w: u32, h: u32) -> Src {
    Src { sheet, x, y, w, h }
}

const SW2: &str = "ui/SystemWindow2_/SystemWindow2.png";
const CW2: &str = "ui/SystemWindow2_/CommonWindow2.png";
const MAIN: &str = "ui/MainMenu_/MainMenu.png";
const UNIT: &str = "ui/MyUnit_/MyUnit.png";
const DIALOG: &str = "ui/SystemWindow_/Dialog.png";
const WALL2: &str = "ui/Wallpaper2_/Wallpaper2.png";
const GRAD: &str = "ui/SystemWindow2_/gradation_black_1.png";
/// info-ui: the lower screen's unit status sheets, the game's UI font (its
/// digits) and the growth-rates button made from the status sheet's own
/// button (dev/info-ui/make_button.py)
const STATUS1: &str = "ui/status_/Status1.png";
const STATUS2: &str = "ui/status_/Status2.png";
const UIFONT: &str = "font/UIFont_/UIFont.png";
const GROWTH_BUTTON: &str = "ui/info-ui/GrowthButton.png";
/// input: SystemWindow2's atlas with its button pictures redrawn for Xbox
/// and PlayStation, at four times the size.
const GLYPHS_XBOX: &str = "glyphs/xbox/tex1_512x256_E6F746B2E422F774_13_mip0.png";
const GLYPHS_PS: &str = "glyphs/playstation/tex1_512x256_E6F746B2E422F774_13_mip0.png";

/// every piece of art the kit draws, by name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Art {
    /// the ornate bar across the top and bottom of the game's menu screens.
    Header,
    /// the notched parchment panel menus sit on (the New Game screen's).
    Panel,
    /// the dark teal wallpaper behind menu panels.
    Wallpaper,
    /// the checkered cream message box ("The game must be initialized").
    Dialog,
    /// the soft red bar behind the selected row.
    Highlight,
    /// the ornate red bar.
    HighlightOrnate,
    /// rounded pills: the dark prompt pill, and MyUnit's label/value pills.
    PillPrompt,
    PillLabel,
    PillCream,
    PillRed,
    PillGray,
    /// the coloured pennant tags (difficulty tags): red, blue, purple,
    /// orange, green, pink.
    Tag(u8),
    /// the pointing hand cursor.
    Hand,
    ArrowLeft,
    ArrowRight,
    ScrollUp,
    ScrollDown,
    /// button glyphs: A B X Y L R START, and a blank round one.
    /// a button picture: the set (0 the game's own 3DS art, 1 Xbox, 2
    /// PlayStation, from the input agent's assets/glyphs, drawn in the same
    /// style over the same atlas) and the slot: A B X Y L R START.
    Glyph(u8, u8),
    /// the avatar screen's buttons: dark green OK, bright green, gray, and
    /// red Back, bright red, gray.
    ButtonGreen,
    ButtonGreenLit,
    ButtonRed,
    ButtonRedLit,
    ButtonGray,
    /// a black gradient for dimming.
    Gradient,
    /// the corner brackets of the name-entry letter cursor.
    Bracket,
    /// info-ui: the unit status' stats window (Str to Res pills), its HP
    /// plate with the card behind it, the growth button (0 off, 1 on, 2
    /// pointed at), a digit (0-9) or "%" (10) of the UI font the stats are
    /// written in, and a digit of the HP plate's large numbers.
    AbilityWindow,
    HpPlate,
    HpPlateDetailed,
    GrowthButton(u8),
    UiGlyph(u8),
    BigDigit(u8),
}

fn source(art: Art) -> Src {
    match art {
        Art::Header => src(SW2, 0, 2, 400, 36),
        Art::Panel => src(MAIN, 2, 6, 332, 172),
        Art::Wallpaper => src(WALL2, 0, 0, 416, 240),
        Art::Dialog => src(DIALOG, 2, 2, 61, 61),
        Art::Highlight => src(MAIN, 426, 3, 84, 34),
        Art::HighlightOrnate => src(MAIN, 4, 193, 144, 34),
        Art::PillPrompt => src(SW2, 224, 68, 24, 16),
        Art::PillLabel => src(UNIT, 185, 32, 38, 24),
        Art::PillCream => src(UNIT, 185, 56, 38, 24),
        Art::PillRed => src(UNIT, 185, 80, 38, 24),
        Art::PillGray => src(UNIT, 185, 104, 38, 24),
        Art::Tag(i) => src(MAIN, 336 + 56 * (i as u32 % 3), if i < 3 { 43 } else { 75 }, 56, 26),
        Art::Hand => src(SW2, 132, 62, 24, 14),
        Art::ArrowLeft => src(SW2, 157, 81, 9, 14),
        Art::ArrowRight => src(SW2, 170, 81, 9, 14),
        Art::ScrollUp => src(SW2, 129, 177, 28, 31),
        Art::ScrollDown => src(SW2, 129, 207, 28, 31),
        Art::Glyph(set, slot) => {
            let sheet = match set {
                1 => GLYPHS_XBOX,
                2 => GLYPHS_PS,
                _ => SW2,
            };
            match slot {
                6 => src(sheet, 98, 40, 27, 16),
                slot => src(sheet, 16 * slot as u32, 40, 16, 16),
            }
        }
        Art::ButtonGreen => src(CW2, 3, 3, 131, 43),
        Art::ButtonGreenLit => src(CW2, 3, 51, 131, 43),
        Art::ButtonGray => src(CW2, 4, 99, 131, 43),
        Art::ButtonRed => src(CW2, 148, 3, 32, 43),
        Art::ButtonRedLit => src(CW2, 148, 51, 32, 43),
        Art::Gradient => src(GRAD, 0, 0, 64, 64),
        Art::Bracket => src(SW2, 272, 40, 16, 16),
        // info-ui
        Art::AbilityWindow => src(STATUS1, 128, 6, 107, 73),
        Art::HpPlate => src(STATUS2, 360, 270, 124, 38),
        Art::HpPlateDetailed => src(STATUS2, 336, 22, 122, 36),
        Art::GrowthButton(i) => src(GROWTH_BUTTON, 0, 25 * i as u32, 39, 23),
        Art::UiGlyph(10) => src(UIFONT, 40, 104, 8, 14),
        Art::UiGlyph(i) => src(UIFONT, 8 * i as u32, 117, 8, 14),
        Art::BigDigit(i) => src(UIFONT, 177 + 17 * i as u32, 30, 17, 25),
    }
}

const ALL_ART: &[Art] = &[
    Art::Header,
    Art::Panel,
    Art::Wallpaper,
    Art::Dialog,
    Art::Highlight,
    Art::HighlightOrnate,
    Art::PillPrompt,
    Art::PillLabel,
    Art::PillCream,
    Art::PillRed,
    Art::PillGray,
    Art::Tag(0),
    Art::Tag(1),
    Art::Tag(2),
    Art::Tag(3),
    Art::Tag(4),
    Art::Tag(5),
    Art::Hand,
    Art::ArrowLeft,
    Art::ArrowRight,
    Art::ScrollUp,
    Art::ScrollDown,
    Art::Glyph(0, 0),
    Art::Glyph(0, 1),
    Art::Glyph(0, 2),
    Art::Glyph(0, 3),
    Art::Glyph(0, 4),
    Art::Glyph(0, 5),
    Art::Glyph(0, 6),
    Art::Glyph(1, 0),
    Art::Glyph(1, 1),
    Art::Glyph(1, 2),
    Art::Glyph(1, 3),
    Art::Glyph(1, 4),
    Art::Glyph(1, 5),
    Art::Glyph(1, 6),
    Art::Glyph(2, 0),
    Art::Glyph(2, 1),
    Art::Glyph(2, 2),
    Art::Glyph(2, 3),
    Art::Glyph(2, 4),
    Art::Glyph(2, 5),
    Art::Glyph(2, 6),
    Art::ButtonGreen,
    Art::ButtonGreenLit,
    Art::ButtonRed,
    Art::ButtonRedLit,
    Art::ButtonGray,
    Art::Gradient,
    Art::Bracket,
    // info-ui
    Art::AbilityWindow,
    Art::HpPlate,
    Art::HpPlateDetailed,
    Art::GrowthButton(0),
    Art::GrowthButton(1),
    Art::GrowthButton(2),
    Art::UiGlyph(0),
    Art::UiGlyph(1),
    Art::UiGlyph(2),
    Art::UiGlyph(3),
    Art::UiGlyph(4),
    Art::UiGlyph(5),
    Art::UiGlyph(6),
    Art::UiGlyph(7),
    Art::UiGlyph(8),
    Art::UiGlyph(9),
    Art::UiGlyph(10),
    Art::BigDigit(0),
    Art::BigDigit(1),
    Art::BigDigit(2),
    Art::BigDigit(3),
    Art::BigDigit(4),
    Art::BigDigit(5),
    Art::BigDigit(6),
    Art::BigDigit(7),
    Art::BigDigit(8),
    Art::BigDigit(9),
];

/// a glyph of the bitmap font, in the font's own pixels.
#[derive(Clone, Copy, Debug)]
struct Glyph {
    uv: Rect,
    size: Vec2,
    bearing: Vec2,
    advance: f32,
}

/// the System_L font, whose ascent the game's lines sit on.
pub const ASCENT: f32 = 14.0;
pub const LINE: f32 = 16.0;

#[derive(serde::Deserialize)]
struct Metric {
    codepoint: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bearing_x: i32,
    bearing_y: i32,
    advance: i32,
}

/// the decoded sheets, kept to rebuild the textures at another factor.
struct Sources {
    sheets: HashMap<&'static str, RgbaImage>,
    font: RgbaImage,
    metrics: Vec<Metric>,
}

/// perf3: one factor's pictures, made (cropped, upscaled, the font packed)
/// on a thread of their own; only handing them to egui is left for the
/// interface's thread.
struct Made {
    factor: u32,
    art: Vec<(Art, ColorImage, Vec2)>,
    atlas: ColorImage,
    glyphs: HashMap<char, Glyph>,
}

/// perf3: one factor's textures, kept once made.
struct Built {
    art: HashMap<Art, (TextureHandle, Vec2)>,
    font: TextureHandle,
    glyphs: HashMap<char, Glyph>,
}

/// perf3: the pictures for `factor` (the CPU work of a change of scale).
fn make(sources: &Sources, factor: u32) -> Made {
    let mut art = Vec::with_capacity(ALL_ART.len());
    for a in ALL_ART {
        let s = source(*a);
        let sheet = &sources.sheets[s.sheet];
        // sheets drawn larger than the game's (the glyph sets, at 4x)
        // hold the same layout scaled
        let k = if s.sheet.starts_with("glyphs/") { (sheet.width() / 512).max(1) } else { 1 };
        let (x, y) = (s.x * k, s.y * k);
        let piece = image::imageops::crop_imm(sheet, x, y, (s.w * k).min(sheet.width() - x), (s.h * k).min(sheet.height() - y)).to_image();
        art.push((*a, upscale(&piece, s.w * factor, s.h * factor), Vec2::new(s.w as f32, s.h as f32)));
    }
    let (atlas, glyphs) = font_atlas(&sources.font, &sources.metrics, factor);
    Made { factor, art, atlas, glyphs }
}

pub struct Kit {
    /// perf3: shared with the thread that makes a factor's pictures ahead
    sources: std::sync::Arc<Sources>,
    /// perf3: every factor made so far, and those being made
    built: HashMap<u32, Built>,
    making: std::collections::HashSet<u32>,
    made: (std::sync::mpsc::Sender<Made>, std::sync::mpsc::Receiver<Made>),
    factor: u32,
    /// hotfix: frames in a row the wanted factor has been below the one
    /// built, so a panel sliding away does not rebuild every texture each
    /// frame (a burst that once ran the presenter out of descriptors).
    lower_for: u32,
    art: HashMap<Art, (TextureHandle, Vec2)>,
    font: Option<TextureHandle>,
    glyphs: HashMap<char, Glyph>,
}

impl Kit {
    /// loads the art from assets/, none when it is missing.
    pub fn load() -> Result<Kit, String> {
        let dir = assets_dir();
        let mut sheets = HashMap::new();
        for art in ALL_ART {
            let sheet = source(*art).sheet;
            if !sheets.contains_key(sheet) {
                let path = dir.join(sheet);
                let image = match image::open(&path) {
                    Ok(image) => image.to_rgba8(),
                    // the other button pictures are optional: the game's own instead
                    // info-ui: a growth button missing from assets/ is the
                    // status sheet's own book button, never an invisible one
                    Err(error) if sheet == GROWTH_BUTTON => {
                        log::warn!("ui: {}: {error}, the growth button is drawn as the stats window's", path.display());
                        let mut button = RgbaImage::new(39, 73);
                        if let Ok(status) = image::open(dir.join(STATUS1)) {
                            let status = status.to_rgba8();
                            for i in 0..3 {
                                let piece = image::imageops::crop_imm(&status, 244, 4 + 32 * i, 39, 23).to_image();
                                image::imageops::replace(&mut button, &piece, 0, 25 * i as i64);
                            }
                        }
                        button
                    }
                    // info-ui: without its sheets the growth view draws nothing
                    Err(error) if sheet == STATUS1 || sheet == STATUS2 || sheet == UIFONT => {
                        log::warn!("ui: {}: {error}, no growth-rates view", path.display());
                        RgbaImage::new(512, 512)
                    }
                    Err(error) if sheet.starts_with("glyphs/") => {
                        log::warn!("ui: {}: {error}, using the game's own button pictures", path.display());
                        image::open(dir.join(SW2)).map_err(|error| format!("{SW2}: {error}"))?.to_rgba8()
                    }
                    Err(error) => return Err(format!("{}: {error}", path.display())),
                };
                sheets.insert(sheet, image);
            }
        }
        let font_path = dir.join("font/System_L_0.png");
        let font = image::open(&font_path).map_err(|error| format!("{}: {error}", font_path.display()))?.to_rgba8();
        let metrics_path = dir.join("font/System_L-metrics.json");
        let text = std::fs::read_to_string(&metrics_path).map_err(|error| format!("{}: {error}", metrics_path.display()))?;
        let metrics: Vec<Metric> = serde_json::from_str(&text).map_err(|error| format!("{}: {error}", metrics_path.display()))?;
        Ok(Kit {
            sources: std::sync::Arc::new(Sources { sheets, font, metrics }),
            built: HashMap::new(),
            making: Default::default(),
            made: std::sync::mpsc::channel(),
            factor: 0,
            lower_for: 0,
            art: HashMap::new(),
            font: None,
            glyphs: HashMap::new(),
        })
    }

    /// perf3: the factor `pixels` physical pixels per unit draws with.
    fn factor_for(pixels: f32) -> u32 {
        (pixels.ceil() as u32).clamp(2, 6)
    }

    /// perf3: makes the pictures for drawing at `pixels` on a thread of its
    /// own, ahead of their first use: the boon/bane panel beside the
    /// avatar's menu draws the art at the top screen's scale (6 at 1440
    /// rows), and making that factor on the interface's thread when the
    /// panel first showed (Catmull-Rom upscales of every piece and the
    /// font's atlas) held the window for about 200 ms.
    pub fn warm(&mut self, pixels: f32) {
        let factor = Self::factor_for(pixels);
        if factor == self.factor || self.built.contains_key(&factor) || !self.making.insert(factor) {
            return;
        }
        let (sources, tx) = (self.sources.clone(), self.made.0.clone());
        let spawned = std::thread::Builder::new().name("ui art".to_owned()).spawn(move || {
            let started = std::time::Instant::now();
            let made = make(&sources, factor);
            log::info!("perf3: the interface's art at {factor}x made ahead in {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
            let _ = tx.send(made);
        });
        if spawned.is_err() {
            self.making.remove(&factor);
        }
    }

    /// perf3: a factor's pictures handed to egui and kept.
    fn upload(&mut self, ctx: &egui::Context, made: Made) {
        let options = TextureOptions { magnification: egui::TextureFilter::Linear, minification: egui::TextureFilter::Linear, ..TextureOptions::LINEAR };
        let mut art = HashMap::new();
        for (a, image, size) in made.art {
            art.insert(a, (ctx.load_texture(format!("fates {a:?}"), image, options), size));
        }
        let font = ctx.load_texture("fates font", made.atlas, options);
        self.making.remove(&made.factor);
        self.built.insert(made.factor, Built { art, font, glyphs: made.glyphs });
    }

    /// makes the textures for drawing at pixels physical pixels per unit,
    /// when the ones there are were made for a scale far from it.
    pub fn prepare(&mut self, ctx: &egui::Context, pixels: f32) {
        // perf3: what the thread made ahead
        while let Ok(made) = self.made.1.try_recv() {
            self.upload(ctx, made);
        }
        let factor = Self::factor_for(pixels);
        if factor == self.factor {
            self.lower_for = 0;
            return;
        }
        // hotfix: larger textures at once (sharper); smaller ones only once
        // the scale has settled lower, textures a size up draw fine meanwhile
        if factor < self.factor {
            self.lower_for += 1;
            if self.lower_for < 90 {
                return;
            }
        }
        self.lower_for = 0;
        self.factor = factor;
        // perf3: a factor made before (or ahead) is used as it is; one not
        // made yet is made here, as before, and kept
        if !self.built.contains_key(&factor) {
            let started = std::time::Instant::now();
            let made = make(&self.sources, factor);
            self.upload(ctx, made);
            log::info!("perf3: the interface's art at {factor}x made when first drawn, {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
        }
        let b = &self.built[&factor];
        self.art = b.art.clone();
        self.font = Some(b.font.clone());
        self.glyphs = b.glyphs.clone();
    }

    /// the art's size in units.
    pub fn size(&self, art: Art) -> Vec2 {
        self.art.get(&art).map_or(Vec2::ZERO, |(_, size)| *size)
    }

    /// width of text in units at scale.
    pub fn measure(&self, text: &str, scale: f32) -> f32 {
        text.chars().map(|c| self.glyph(c).map_or(0.0, |g| g.advance)).sum::<f32>() * scale
    }

    fn glyph(&self, c: char) -> Option<&Glyph> {
        self.glyphs.get(&c).or_else(|| self.glyphs.get(&'?'))
    }

    /// splits text into as few lines as wrap does, as even in length as
    /// they go, so a last word is never left alone on its line.
    pub fn wrap_even(&self, text: &str, width: f32, scale: f32) -> Vec<String> {
        let mut best = self.wrap(text, width, scale);
        let count = best.len();
        let mut narrower = width;
        while count > 1 && narrower > width / 2.0 {
            narrower -= 4.0;
            let lines = self.wrap(text, narrower, scale);
            if lines.len() != count {
                break;
            }
            best = lines;
        }
        best
    }

    /// splits text into lines no wider than width units.
    pub fn wrap(&self, text: &str, width: f32, scale: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            for word in paragraph.split(' ') {
                let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
                if !line.is_empty() && self.measure(&candidate, scale) > width {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_owned();
                } else {
                    line = candidate;
                }
            }
            lines.push(line);
        }
        lines
    }
}

/// a place to draw in units: the painter, where unit (0, 0) is, and how many
/// points a unit is.
#[derive(Clone)]
pub struct Canvas<'a> {
    pub kit: &'a Kit,
    pub painter: Painter,
    pub origin: Pos2,
    pub unit: f32,
    /// physical pixels per point, for snapping text to whole pixels.
    pub ppp: f32,
    /// multiplies every colour drawn, for fading in and out.
    pub alpha: f32,
}

impl<'a> Canvas<'a> {
    pub fn point(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(self.origin.x + x * self.unit, self.origin.y + y * self.unit)
    }

    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.point(x, y), Vec2::new(w * self.unit, h * self.unit))
    }

    /// the point back in units.
    pub fn unit_of(&self, pos: Pos2) -> (f32, f32) {
        ((pos.x - self.origin.x) / self.unit, (pos.y - self.origin.y) / self.unit)
    }

    fn tint(&self, color: Color32) -> Color32 {
        if self.alpha >= 1.0 {
            color
        } else {
            color.gamma_multiply(self.alpha.max(0.0))
        }
    }

    /// a piece of art stretched over x, y, w, h.
    pub fn image(&self, art: Art, x: f32, y: f32, w: f32, h: f32, tint: Color32) {
        self.image_uv(art, self.rect(x, y, w, h), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
    }

    /// a piece of art at its own size.
    pub fn sprite(&self, art: Art, x: f32, y: f32, tint: Color32) {
        let size = self.kit.size(art);
        self.image(art, x, y, size.x, size.y, tint);
    }

    pub(crate) fn image_uv(&self, art: Art, rect: Rect, uv: Rect, tint: Color32) {
        let Some((texture, _)) = self.kit.art.get(&art) else { return };
        let mut mesh = Mesh::with_texture(texture.id());
        mesh.add_rect_with_uv(rect, uv, self.tint(tint));
        self.painter.add(mesh);
    }

    /// a piece of art as a nine-slice over x, y, w, h: its corners as they
    /// are, by border units (left, top, right, bottom) of the art, its
    /// edges and middle stretched. scale sizes the corners.
    #[allow(clippy::too_many_arguments)]
    pub fn nine(&self, art: Art, x: f32, y: f32, w: f32, h: f32, border: [f32; 4], scale: f32, tint: Color32) {
        let Some((texture, size)) = self.kit.art.get(&art) else { return };
        let [l, t, r, b] = border;
        // corners never overlap however small the box is
        let fit = (w / ((l + r) * scale)).min(h / ((t + b) * scale)).min(1.0);
        let (dl, dt, dr, db) = (l * scale * fit, t * scale * fit, r * scale * fit, b * scale * fit);
        let xs = [x, x + dl, x + w - dr, x + w];
        let ys = [y, y + dt, y + h - db, y + h];
        let us = [0.0, l / size.x, 1.0 - r / size.x, 1.0];
        let vs = [0.0, t / size.y, 1.0 - b / size.y, 1.0];
        let tint = self.tint(tint);
        let mut mesh = Mesh::with_texture(texture.id());
        for j in 0..3 {
            for i in 0..3 {
                if xs[i + 1] <= xs[i] || ys[j + 1] <= ys[j] {
                    continue;
                }
                let rect = Rect::from_min_max(self.point(xs[i], ys[j]), self.point(xs[i + 1], ys[j + 1]));
                let uv = Rect::from_min_max(Pos2::new(us[i], vs[j]), Pos2::new(us[i + 1], vs[j + 1]));
                mesh.add_rect_with_uv(rect, uv, tint);
            }
        }
        self.painter.add(mesh);
    }

    /// a piece of art as a three-slice across: its ends as they are, the
    /// middle stretched, over the full height.
    pub fn three(&self, art: Art, x: f32, y: f32, w: f32, h: f32, ends: [f32; 2], tint: Color32) {
        let size = self.kit.size(art);
        let scale = h / size.y.max(1.0);
        self.nine(art, x, y, w, h, [ends[0], 0.0, ends[1], 0.0], scale, tint);
    }

    /// a plain filled box, for dimming.
    pub fn fill(&self, x: f32, y: f32, w: f32, h: f32, color: Color32) {
        self.painter.rect_filled(self.rect(x, y, w, h), 0.0, self.tint(color));
    }

    /// text with its top left at x, y, its baseline ASCENT below at scale 1.
    /// returns the width in units.
    pub fn text(&self, x: f32, y: f32, text: &str, scale: f32, color: Color32) -> f32 {
        let Some(font) = &self.kit.font else { return 0.0 };
        let mut mesh = Mesh::with_texture(font.id());
        let color = self.tint(color);
        let snap = |v: f32| (v * self.ppp).round() / self.ppp;
        let baseline = y + ASCENT * scale;
        let mut pen = x;
        for c in text.chars() {
            let Some(glyph) = self.kit.glyph(c) else { continue };
            if glyph.size.x > 1.0 {
                let top_left = self.point(pen + glyph.bearing.x * scale, baseline - glyph.bearing.y * scale);
                let top_left = Pos2::new(snap(top_left.x), snap(top_left.y));
                let rect = Rect::from_min_size(top_left, glyph.size * scale * self.unit);
                mesh.add_rect_with_uv(rect, glyph.uv, color);
            }
            pen += glyph.advance * scale;
        }
        self.painter.add(mesh);
        pen - x
    }

    /// text centred on x.
    pub fn text_centered(&self, x: f32, y: f32, text: &str, scale: f32, color: Color32) {
        let width = self.kit.measure(text, scale);
        self.text(x - width / 2.0, y, text, scale, color);
    }

    /// text ending at x.
    pub fn text_right(&self, x: f32, y: f32, text: &str, scale: f32, color: Color32) {
        let width = self.kit.measure(text, scale);
        self.text(x - width, y, text, scale, color);
    }
}

/// a sheet piece, scaled to width by height in premultiplied alpha.
fn upscale(piece: &RgbaImage, width: u32, height: u32) -> ColorImage {
    let mut premultiplied = piece.clone();
    for pixel in premultiplied.pixels_mut() {
        let a = pixel[3] as u32;
        for c in 0..3 {
            pixel[c] = ((pixel[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let big = image::imageops::resize(&premultiplied, width, height, image::imageops::FilterType::CatmullRom);
    let pixels = big
        .pixels()
        .map(|p| {
            let a = p[3];
            Color32::from_rgba_premultiplied(p[0].min(a), p[1].min(a), p[2].min(a), a)
        })
        .collect();
    ColorImage::new([big.width() as usize, big.height() as usize], pixels)
}

/// the font's glyphs, each upscaled on its own with a pixel of space so
/// they never bleed into each other, packed into one texture. The sheet
/// holds coverage as luminance.
fn font_atlas(sheet: &RgbaImage, metrics: &[Metric], factor: u32) -> (ColorImage, HashMap<char, Glyph>) {
    const WIDTH: u32 = 2048;
    let pad = 1u32;
    // place the glyphs in rows
    let mut placed = Vec::new();
    let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
    for metric in metrics {
        let w = (metric.width + 2 * pad) * factor;
        let h = (metric.height + 2 * pad) * factor;
        if x + w > WIDTH {
            x = 0;
            y += row;
            row = 0;
        }
        placed.push((x, y));
        x += w;
        row = row.max(h);
    }
    let height = (y + row).next_power_of_two().max(64);
    let mut atlas = vec![Color32::TRANSPARENT; (WIDTH * height) as usize];
    let mut glyphs = HashMap::new();
    for (metric, &(ax, ay)) in metrics.iter().zip(&placed) {
        let Some(c) = char::from_u32(metric.codepoint) else { continue };
        let (w, h) = (metric.width + 2 * pad, metric.height + 2 * pad);
        let mut cell = RgbaImage::new(w, h);
        for gy in 0..metric.height {
            for gx in 0..metric.width {
                let (sx, sy) = (metric.x + gx, metric.y + gy);
                if sx < sheet.width() && sy < sheet.height() {
                    let p = sheet.get_pixel(sx, sy);
                    let coverage = p[0].max(p[1]).max(p[2]);
                    cell.put_pixel(gx + pad, gy + pad, image::Rgba([coverage, coverage, coverage, coverage]));
                }
            }
        }
        let big = image::imageops::resize(&cell, w * factor, h * factor, image::imageops::FilterType::CatmullRom);
        for (bx, by, p) in big.enumerate_pixels() {
            // a gentle contrast curve on the coverage keeps edges crisp
            // without thinning the strokes the way a threshold would
            let a = p[3] as f32 / 255.0;
            let a = ((a - 0.5) * 1.35 + 0.5).clamp(0.0, 1.0);
            let a = (a * 255.0).round() as u8;
            atlas[((ay + by) * WIDTH + ax + bx) as usize] = Color32::from_rgba_premultiplied(a, a, a, a);
        }
        let (u0, v0) = ((ax + pad * factor) as f32 / WIDTH as f32, (ay + pad * factor) as f32 / height as f32);
        let (u1, v1) = (u0 + (metric.width * factor) as f32 / WIDTH as f32, v0 + (metric.height * factor) as f32 / height as f32);
        glyphs.insert(
            c,
            Glyph {
                uv: Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)),
                size: Vec2::new(metric.width as f32, metric.height as f32),
                bearing: Vec2::new(metric.bearing_x as f32, metric.bearing_y as f32),
                advance: metric.advance as f32,
            },
        );
    }
    (ColorImage::new([WIDTH as usize, height as usize], atlas), glyphs)
}
