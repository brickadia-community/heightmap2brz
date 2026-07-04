use brdb::{
    AsBrdbValue, BrdbSchemaError, Brick, BrickSize, BrickType, Collision, Color, Position,
    PrefabJson, SavedBrickColor, Vector3f, World,
    assets::{LiteralComponent, bricks::PB_DEFAULT_MICRO_BRICK},
    schema::{BrdbInterned, BrdbSchema, BrdbValue},
};
use image::RgbaImage;

/// Maximum characters (Unicode chars, not bytes) a TextDisplay component accepts.
pub const MAX_COMPONENT_CHARS: usize = 10_000;

/// Calibrated font presets for the text renderer. Each preset carries the
/// glyph scheme and the component geometry (LineHeight/LineOffset/Kerning/
/// Offset) tuned in-game for that font; values scale with pixel size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontPreset {
    /// `██` / two spaces per pixel is square; monospace, so space-based
    /// transparency lines up.
    MonaspaceArgon,
    /// `██` / two spaces per pixel is square; monospace. Calibrated from the
    /// user's reference clipboards (2026-07-04).
    IosevkaTerm,
    /// Single `█` per pixel is square, halving the char budget — but the font
    /// is proportional, so space-based transparency does NOT line up.
    Orbitron,
}

impl FontPreset {
    pub const ALL: [FontPreset; 3] = [
        FontPreset::MonaspaceArgon,
        FontPreset::IosevkaTerm,
        FontPreset::Orbitron,
    ];

    /// Display name for UIs.
    pub fn name(&self) -> &'static str {
        match self {
            FontPreset::MonaspaceArgon => "Monaspace Argon",
            FontPreset::IosevkaTerm => "Iosevka Term",
            FontPreset::Orbitron => "Orbitron",
        }
    }

    /// BrickFontDescriptor asset name.
    pub fn font_asset(&self) -> &'static str {
        match self {
            FontPreset::MonaspaceArgon => "MonaspaceArgon",
            FontPreset::IosevkaTerm => "IosevkaTerm",
            FontPreset::Orbitron => "Orbitron",
        }
    }

    /// Calibrated options for this font at the given pixel size (world units
    /// per pixel row). Geometry values may need manual re-calibration; UIs
    /// expose them for tweaking after the preset seeds them.
    pub fn options(&self, pixel_size: f32) -> TextOptions {
        let base = TextOptions {
            font: self.font_asset(),
            fill_char: '█',
            empty_char: ' ',
            char_repeat: 2,
            alpha_threshold: 128,
            line_world_height: pixel_size,
            line_height: 0.61 * pixel_size,
            line_offset: 0.0,
            kerning: 0.0,
            offset_x: 0.0,
            offset_y: -0.2 * pixel_size,
            // one cube depth: text sits in front of the anchor wall
            offset_z: 2.0,
            pitch_scale: 1.0,
        };
        match self {
            // at LineHeight 0.61 Monaspace renders 30/32 of the nominal
            // pixel size (measured in-game via uniform one-cube tile gaps):
            // the brick grid spacing shrinks to match the rendering
            FontPreset::MonaspaceArgon => TextOptions {
                pitch_scale: 30.0 / 32.0,
                ..base
            },
            FontPreset::IosevkaTerm => base,
            // from the user's "Text Pixel" prefab (2 units/px: LineHeight 1.6,
            // LineOffset -8.5) and F-shape clipboard (0.5 units/px: LineHeight
            // 0.4, LineOffset -8, Kerning -0.1, Offset.Y -0.05)
            FontPreset::Orbitron => TextOptions {
                char_repeat: 1,
                line_height: 0.8 * pixel_size,
                line_offset: -8.0,
                kerning: -0.2 * pixel_size,
                offset_y: -0.1 * pixel_size,
                ..base
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextOptions {
    /// BrickFontDescriptor asset name.
    pub font: &'static str,
    pub fill_char: char,
    pub empty_char: char,
    pub char_repeat: usize,
    pub alpha_threshold: u8,
    /// World units per pixel row (pixel size).
    pub line_world_height: f32,
    /// Component LineHeight (font size).
    pub line_height: f32,
    /// Component LineOffset.
    pub line_offset: f32,
    /// Component Kerning.
    pub kerning: f32,
    /// Component Offset.X — glyph-fit nudge.
    pub offset_x: f32,
    /// Component Offset.Y — glyph-fit nudge.
    pub offset_y: f32,
    /// Component Offset.Z — out-of-plane: pushes the text off the anchor
    /// wall's face so the cubes hide behind the image.
    pub offset_z: f32,
    /// Rendered size / nominal pixel size: the brick grid's tile spacing is
    /// `TILE_PX × line_world_height × pitch_scale`, matching how large the
    /// font actually draws a tile (calibrated in-game from tile gaps:
    /// gaps ⇒ lower, overlaps ⇒ raise).
    pub pitch_scale: f32,
}

impl Default for TextOptions {
    fn default() -> Self {
        FontPreset::MonaspaceArgon.options(1.0)
    }
}

/// One TextDisplay component's worth of image rows. Bands after the first
/// begin with `start_row` newlines so the game's own line advance places
/// their rows at the exact image depth (no measured-advance error).
#[derive(Clone, Debug)]
pub struct TextBand {
    /// First image row rendered by this band (also its padding-line count).
    pub start_row: usize,
    /// Number of image rows in this band (excluding padding lines).
    pub rows: usize,
    pub text: String,
    /// Unicode char count of `text`, including padding newlines.
    pub chars: usize,
}

pub fn encode_bands(img: &RgbaImage, opts: &TextOptions) -> Result<Vec<TextBand>, String> {
    let (_, h) = img.dimensions();
    let mut bands: Vec<TextBand> = Vec::new();
    let mut cur = TextBand {
        start_row: 0,
        rows: 0,
        text: String::new(),
        chars: 0,
    };
    // Last emitted <color> tag; persists across pixels, gaps, and rows within a band.
    let mut last_color: Option<[u8; 3]> = None;

    for y in 0..h {
        let mut row_state = last_color;
        let (row_text, row_chars) = encode_row(img, y, opts, &mut row_state);
        let sep = if cur.rows == 0 { 0 } else { 1 };

        if cur.rows > 0 && cur.chars + sep + row_chars > MAX_COMPONENT_CHARS {
            // Close the band; the next one starts with y padding newlines
            // (the game's own line advance shifts its rows into place) and
            // fresh color state since a new component starts colorless.
            bands.push(std::mem::replace(
                &mut cur,
                TextBand {
                    start_row: y as usize,
                    rows: 0,
                    text: "\n".repeat(y as usize),
                    chars: y as usize,
                },
            ));
            let mut fresh = None;
            let (row_text, row_chars) = encode_row(img, y, opts, &mut fresh);
            if row_chars > MAX_COMPONENT_CHARS {
                return Err(row_too_wide(y, row_chars));
            }
            if cur.chars + row_chars > MAX_COMPONENT_CHARS {
                return Err(format!(
                    "row {y}: {} padding lines plus a {row_chars}-char row exceed the \
                     {MAX_COMPONENT_CHARS}-char TextDisplay limit; the image is too tall \
                     to band",
                    cur.chars
                ));
            }
            cur.text.push_str(&row_text);
            cur.chars += row_chars;
            cur.rows = 1;
            last_color = fresh;
            continue;
        }
        if row_chars > MAX_COMPONENT_CHARS {
            return Err(row_too_wide(y, row_chars));
        }
        if sep == 1 {
            cur.text.push('\n');
            cur.chars += 1;
        }
        cur.text.push_str(&row_text);
        cur.chars += row_chars;
        cur.rows += 1;
        last_color = row_state;
    }
    if cur.rows > 0 || bands.is_empty() {
        bands.push(cur);
    }
    Ok(bands)
}

/// Square pixel patch tiled across the image in both axes. Small enough
/// that even a worst-case patch (a color tag on every pixel) fits one or
/// two components, and each patch anchors to its own nearby cube so
/// component offsets stay tiny.
pub const TILE_PX: u32 = 32;

/// One tile of the image: a `TILE_PX`-square pixel patch (smaller at the
/// right/bottom edges) with its own bands.
#[derive(Clone, Debug)]
pub struct TextTile {
    /// First pixel column of this tile.
    pub start_col: usize,
    /// First pixel row of this tile.
    pub start_row: usize,
    pub bands: Vec<TextBand>,
}

/// Tile the image into `TILE_PX`-square patches across BOTH axes, banding
/// each patch at the char budget (worst-case patches split into a couple of
/// bands; typical patches are one). Fully transparent patches are skipped
/// entirely — no brick, no component.
pub fn encode_tiles(img: &RgbaImage, opts: &TextOptions) -> Result<Vec<TextTile>, String> {
    let (w, h) = img.dimensions();
    let mut tiles = Vec::new();
    let mut ty = 0u32;
    while ty < h {
        let th = TILE_PX.min(h - ty);
        let mut tx = 0u32;
        while tx < w {
            let tw = TILE_PX.min(w - tx);
            let sub = image::imageops::crop_imm(img, tx, ty, tw, th).to_image();
            let bands = encode_bands(&sub, opts)?;
            // a patch whose bands hold no glyphs at all is invisible
            if bands.iter().any(|b| b.text.chars().any(|c| c != '\n')) {
                tiles.push(TextTile {
                    start_col: tx as usize,
                    start_row: ty as usize,
                    bands,
                });
            }
            tx += tw;
        }
        ty += th;
    }
    Ok(tiles)
}

fn row_too_wide(y: u32, chars: usize) -> String {
    format!(
        "row {y} encodes to {chars} chars, over the {MAX_COMPONENT_CHARS}-char \
         TextDisplay limit; use a narrower image or a smaller --char-repeat"
    )
}

/// Encode one image row. Updates `last_color` with the final emitted tag so
/// color runs can continue into following rows.
fn encode_row(
    img: &RgbaImage,
    y: u32,
    opts: &TextOptions,
    last_color: &mut Option<[u8; 3]>,
) -> (String, usize) {
    let (w, _) = img.dimensions();
    let mut out = String::new();
    let mut chars = 0usize;
    // Transparent pixels are buffered so a trailing run can be trimmed when the
    // empty glyph is a space (invisible at line end); any other glyph is kept.
    let mut pending_empty = 0usize;
    for x in 0..w {
        let p = img.get_pixel(x, y).0;
        if p[3] < opts.alpha_threshold {
            pending_empty += 1;
            continue;
        }
        for _ in 0..pending_empty * opts.char_repeat {
            out.push(opts.empty_char);
        }
        chars += pending_empty * opts.char_repeat;
        pending_empty = 0;

        let rgb = [p[0], p[1], p[2]];
        if *last_color != Some(rgb) {
            let tag = format!("<color=\"{:02X}{:02X}{:02X}\">", rgb[0], rgb[1], rgb[2]);
            chars += tag.chars().count();
            out.push_str(&tag);
            *last_color = Some(rgb);
        }
        for _ in 0..opts.char_repeat {
            out.push(opts.fill_char);
        }
        chars += opts.char_repeat;
    }
    if opts.empty_char != ' ' {
        for _ in 0..pending_empty * opts.char_repeat {
            out.push(opts.empty_char);
        }
        chars += pending_empty * opts.char_repeat;
    }
    (out, chars)
}

/// brdb ships `Vector3f` but no 2-component wrapper; the TextDisplay `Anchor`
/// field is a `Vector2f` struct in the component schema.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector2f {
    pub x: f32,
    pub y: f32,
}

impl AsBrdbValue for Vector2f {
    fn as_brdb_struct_prop_value(
        &self,
        schema: &BrdbSchema,
        _struct_name: BrdbInterned,
        prop_name: BrdbInterned,
    ) -> Result<&dyn AsBrdbValue, BrdbSchemaError> {
        match prop_name.get(schema).unwrap() {
            "X" => Ok(&self.x),
            "Y" => Ok(&self.y),
            n => unimplemented!("unimplemented Vector2f field {n}"),
        }
    }
}

/// Measured font geometry, in world units per LineHeight unit. Read these
/// off the measuring save's rulers once per font; every pixel size then
/// solves exactly instead of being nudged iteratively.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    /// Vertical world units the game advances per text line, per LineHeight.
    pub line_advance: f32,
    /// Horizontal world units one `█` advances (kerning 0), per LineHeight.
    pub char_advance: f32,
}

impl FontMetrics {
    /// Rows/columns in the measuring block (measured at LineHeight 1.0).
    pub const MEASURE_ROWS: usize = 10;

    /// Metrics from the measuring block's spans in world units: the height
    /// of its 10 rows and the width of its 10 single-`█` columns.
    pub fn from_measured(vertical_span: f32, horizontal_span: f32) -> Self {
        Self {
            line_advance: vertical_span / Self::MEASURE_ROWS as f32,
            char_advance: horizontal_span / Self::MEASURE_ROWS as f32,
        }
    }

    /// Solve (LineHeight, Kerning) so one pixel is exactly `pixel_size`
    /// world units tall and `char_repeat` glyphs are exactly as wide.
    pub fn solve(&self, pixel_size: f32, char_repeat: usize) -> (f32, f32) {
        let line_height = pixel_size / self.line_advance;
        let kerning =
            pixel_size / char_repeat.max(1) as f32 - self.char_advance * line_height;
        (line_height, kerning)
    }
}

/// The invisible, collision-less anchor cube all text rides on.
fn anchor_cube(position: Position, visible: bool) -> Brick {
    Brick {
        asset: BrickType::Procedural {
            asset: PB_DEFAULT_MICRO_BRICK,
            size: BrickSize::new(1, 1, 1),
        },
        position,
        collision: Collision {
            player: false,
            weapon: false,
            interact: false,
            tool: false,
            physics: false,
            ..Default::default()
        },
        visible,
        ..Default::default()
    }
}

/// Add a TextDisplay block with explicit geometry (LineHeight/Kerning/Offset)
/// on an anchor cube. `visible_anchor` shows the cube itself, useful when the
/// block is meant to be compared against the cube's edges.
pub fn add_text_block(
    world: &mut World,
    text: String,
    position: Position,
    line_height: f32,
    kerning: f32,
    offset: Vector3f,
    visible_anchor: bool,
    opts: &TextOptions,
) {
    let (font_idx, _) = world
        .global_data
        .external_asset_references
        .insert_full(("BrickFontDescriptor".to_string(), opts.font.to_string()));
    let block_opts = TextOptions {
        line_height,
        kerning,
        ..opts.clone()
    };
    world.add_brick(anchor_cube(position, visible_anchor).with_component(
        text_display_component(text, font_idx, offset, &block_opts),
    ));
}

/// Add a readable TextDisplay label brick (for annotating generated saves) —
/// plain text at the given LineHeight, not pixel art.
pub fn add_annotation(
    world: &mut World,
    text: String,
    position: Position,
    line_height: f32,
    opts: &TextOptions,
) {
    add_text_block(
        world,
        text,
        position,
        line_height,
        0.0,
        Vector3f { x: 0.0, y: 0.0, z: 0.0 },
        false,
        opts,
    );
}

/// A visible ruler cube (2 world units) for measuring against the stud grid.
fn ruler_cube(position: Position, red: bool) -> Brick {
    let mut b = anchor_cube(position, true);
    b.color = if red {
        Color { r: 255, g: 60, b: 60 }
    } else {
        Color { r: 250, g: 250, b: 250 }
    };
    b
}

/// Build the font-measuring save. Layout (all cubes at non-negative z):
///
/// - A 10×10 `█` measuring block at LineHeight 1.0 / Kerning 0 / Offset 0,
///   with a vertical and horizontal ruler of alternating 2-unit cubes along
///   its anchor edges. Reading the block's spans off the rulers gives
///   [`FontMetrics`] directly (the GUI solves the rest).
/// - A LineHeight sweep: five 8×8 checker blocks around the current solved
///   LineHeight, each with a visible anchor cube and a marker cube at the
///   expected bottom-right corner — the candidate whose checker exactly
///   meets its marker is correct.
/// - An Offset-Y sweep: five single-row strips on visible anchor cubes;
///   pick the one whose glyph top aligns with its cube top.
pub fn build_measuring_world(opts: &TextOptions) -> World {
    const N: usize = FontMetrics::MEASURE_ROWS;
    let px = opts.line_world_height;
    let fill: String = opts.fill_char.to_string();
    let fill_px: String = fill.repeat(opts.char_repeat.max(1));
    let mut world = World::new();

    // ---- measuring block (single glyph per column, LineHeight 1.0) ----
    let top = 90i32;
    let measure_text = (0..N)
        .map(|y| {
            let c = if y % 2 == 0 { "FF3C3C" } else { "FAFAFA" };
            format!("<color=\"{c}\">{}", fill.repeat(N))
        })
        .collect::<Vec<_>>()
        .join("\n");
    add_text_block(
        &mut world,
        measure_text,
        Position::new(0, 0, top),
        1.0,
        0.0,
        Vector3f { x: 0.0, y: 0.0, z: 0.0 },
        true,
        opts,
    );
    // rulers: vertical below the anchor, horizontal to its right
    for i in 0..14i32 {
        world.add_brick(ruler_cube(Position::new(0, 0, top - 2 - 2 * i), i % 2 == 0));
        world.add_brick(ruler_cube(Position::new(2 + 2 * i, 0, top), i % 2 == 0));
    }
    add_annotation(
        &mut world,
        format!(
            "MEASURE ({}): block is {N} rows x {N} single-glyph columns at LineHeight 1.\n\
             Read its height and width in world units off the rulers (1 cube = 2 units),\n\
             enter both in the GUI (Measured V / Measured H) and hit Apply.",
            opts.font
        ),
        Position::new(0, 0, top + 8),
        1.2,
        opts,
    );

    // ---- LineHeight sweep: 8x8 checkers with expected-corner markers ----
    let sweep_z = 44i32;
    let checker_px = 8usize;
    let checker_text = (0..checker_px)
        .map(|y| {
            (0..checker_px)
                .map(|x| {
                    let c = if (x + y) % 2 == 0 { "FF3C3C" } else { "FAFAFA" };
                    format!("<color=\"{c}\">{fill_px}")
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let expected = (checker_px as f32 * px).round() as i32;
    for (i, scale) in [0.94f32, 0.97, 1.0, 1.03, 1.06].into_iter().enumerate() {
        let lh = opts.line_height * scale;
        let x = (i as f32 * (checker_px as f32 * px + 14.0)).round() as i32;
        add_text_block(
            &mut world,
            checker_text.clone(),
            Position::new(x, 0, sweep_z),
            lh,
            opts.kerning,
            Vector3f { x: opts.offset_x, y: opts.offset_y, z: 0.0 },
            true,
            opts,
        );
        // marker cube whose top-left corner is the checker's expected
        // bottom-right corner
        world.add_brick(ruler_cube(
            Position::new(x + expected + 1, 0, sweep_z - expected - 1),
            true,
        ));
        add_annotation(
            &mut world,
            format!("LH {lh:.3}"),
            Position::new(x, 0, sweep_z + 4),
            1.0,
            opts,
        );
    }
    add_annotation(
        &mut world,
        "LINEHEIGHT SWEEP: pick the checker that exactly meets its corner marker."
            .to_string(),
        Position::new(0, 0, sweep_z + 8),
        1.2,
        opts,
    );

    // ---- Offset-Y sweep: glyph top vs cube top ----
    let off_z = 12i32;
    for (i, delta) in [-0.2f32, -0.1, 0.0, 0.1, 0.2].into_iter().enumerate() {
        let off = opts.offset_y + delta * px;
        let x = (i as f32 * (4.0 * px + 12.0)).round() as i32;
        add_text_block(
            &mut world,
            format!("<color=\"FF3C3C\">{}", fill_px.repeat(4)),
            Position::new(x, 0, off_z),
            opts.line_height,
            opts.kerning,
            Vector3f { x: opts.offset_x, y: off, z: 0.0 },
            true,
            opts,
        );
        add_annotation(
            &mut world,
            format!("OffY {off:.3}"),
            Position::new(x, 0, off_z + 4),
            1.0,
            opts,
        );
    }
    add_annotation(
        &mut world,
        "OFFSET SWEEP: pick the strip whose glyph top sits flush with its cube top."
            .to_string(),
        Position::new(0, 0, off_z + 8),
        1.2,
        opts,
    );

    world.meta.bundle.description = "Text render font measuring tool".to_string();
    world.make_prefab();
    // brdb 0.6 requires used component types registered before writing;
    // add_text_bricks does this itself but the measuring world is built from
    // raw text blocks
    world.register_used_components();
    world
}

/// Mark the world as a prefab whose pivots/bounds cover the full rendered
/// image. The anchor-cube grid already spans most of the image; each
/// outermost tile's content extends up to one tile beyond its cube toward
/// the text's right (world -Y) and downward (-Z), so the bounds pad those
/// two edges — a ground placement then rests the image (not the cubes) on
/// the ground.
pub fn make_text_prefab(world: &mut World, _img_w: u32, _img_h: u32, opts: &TextOptions) {
    let tile_span = (TILE_PX as f32 * opts.line_world_height * opts.pitch_scale).ceil() as i32;
    let (bmin, bmax) = world
        .brick_bounds()
        .unwrap_or((Position::ZERO, Position::ZERO));
    let min = Position::new(bmin.x, bmin.y - tile_span, bmin.z - tile_span);
    world.meta.bundle.level_type = "Prefab".to_string();
    world.meta.prefab = Some(PrefabJson::from_bounds(min, bmax));
}

/// A 1×1×1 micro brick is 2 world units tall; the anchor cubes pack into a
/// flush vertical column instead of spreading out one band-height apart.
const BRICK_STACK_STEP: i32 = 2;

/// Add one TextDisplay micro brick per band to the world, as a single tile
/// at the origin. See [`add_text_tiles`] for tiled images.
pub fn add_text_bricks(world: &mut World, bands: Vec<TextBand>, opts: &TextOptions) {
    add_text_tiles(
        world,
        vec![TextTile {
            start_col: 0,
            start_row: 0,
            bands,
        }],
        opts,
    );
}

/// Add all tiles' bricks. Each tile's anchor cube sits AT its patch's image
/// position (brick coordinates carry ALL the placement — the game does not
/// honor large component Offset values), forming a sparse cube grid with
/// the same footprint as the image, always covered by the rendered text.
/// A multi-band tile stacks its extra cubes upward from the tile anchor
/// (staying non-negative for the bottom row), each band's text shifting
/// back down by its cube's displacement. Component offsets carry only the
/// glyph nudges and that tiny in-tile compensation.
pub fn add_text_tiles(world: &mut World, tiles: Vec<TextTile>, opts: &TextOptions) {
    // Index into global_data.external_asset_references, written as the Font
    // object reference (same pattern bearilog uses for item assets).
    let (font_idx, _) = world
        .global_data
        .external_asset_references
        .insert_full(("BrickFontDescriptor".to_string(), opts.font.to_string()));

    // grid spacing follows the font's ACTUAL rendered size, not the nominal
    // pixel size — tiles anchor exactly where their neighbors' glyphs end
    let pitch = opts.line_world_height * opts.pitch_scale;
    let max_row = tiles.iter().map(|t| t.start_row).max().unwrap_or(0) as f32;
    // brdb's chunk encoding mishandles negative coordinates (bricks land in
    // wrong chunks in-game), so the grid is translated to keep EVERY brick
    // coordinate non-negative: the rightmost tile sits at y=0 and the image
    // grows toward +Y (the text's right is world -Y, so the order mirrors).
    let max_col_span = tiles
        .iter()
        .map(|t| (t.start_col as f32 * pitch).round() as i32)
        .max()
        .unwrap_or(0);
    let mut bricks = Vec::new();
    for tile in tiles {
        let y = max_col_span - (tile.start_col as f32 * pitch).round() as i32;
        // bottom tile row anchors at z=1; rows above it proportionally higher
        let z_tile = 1 + ((max_row - tile.start_row as f32) * pitch).round() as i32;
        for (i, band) in tile.bands.into_iter().enumerate() {
            let i = i as i32;
            let offset = Vector3f {
                x: opts.offset_x,
                // extra band cubes sit above the anchor; shift their text
                // back down (negative Y = down)
                y: opts.offset_y - (BRICK_STACK_STEP * i) as f32,
                z: opts.offset_z,
            };
            bricks.push(
                anchor_cube(Position::new(0, y, z_tile + BRICK_STACK_STEP * i), false)
                    .with_component(text_display_component(band.text, font_idx, offset, opts)),
            );
        }
    }
    world.add_bricks(bricks);
    // brdb 0.6 requires used component types registered in global data before
    // writing; rebuilds from the max schema, so calling once at the end is fine.
    world.register_used_components();
}

/// TextDisplay component data mirroring the user's calibrated reference
/// clipboards: Anchor top-left, glyph-fit Offset, LineHeight sized so pixel
/// rows land on world units — all seeded by the selected [`FontPreset`].
fn text_display_component(
    text: String,
    font_idx: usize,
    offset: Vector3f,
    opts: &TextOptions,
) -> LiteralComponent {
    LiteralComponent::new("Component_TextDisplay").with_data([
        ("Text", Box::new(text) as Box<dyn AsBrdbValue>),
        ("Font", Box::new(BrdbValue::Asset(Some(font_idx)))),
        ("Anchor", Box::new(Vector2f { x: 0.0, y: 0.0 })),
        ("Offset", Box::new(offset)),
        ("Rotation", Box::new(0.0f32)),
        ("Skew", Box::new(0.0f32)),
        ("Kerning", Box::new(opts.kerning)),
        ("LineHeight", Box::new(opts.line_height)),
        ("LineOffset", Box::new(opts.line_offset)),
        (
            "Color",
            Box::new(SavedBrickColor {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
        ),
        ("MaterialSlider", Box::new(10i32)),
        ("ShadingWidth", Box::new(2.0f32)),
        (
            "OutlineColor",
            Box::new(SavedBrickColor {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
        ),
        ("OutlineWidth", Box::new(2.0f32)),
        ("ScuffWidth", Box::new(0.0f32)),
        ("GraffitiDepthLimit", Box::new(5.0f32)),
        ("GraffitiAngleLimit", Box::new(45.0f32)),
        ("GraffitiLayer", Box::new(0i32)),
        ("bEnabled", Box::new(true)),
        ("Face", Box::new(0u8)),
        ("bAlignToWedge", Box::new(false)),
        ("bOverrideColor", Box::new(true)),
        ("Typeface", Box::new(0u8)),
        ("Billboard", Box::new(0u8)),
        ("Material", Box::new(0u8)),
        ("Shading", Box::new(0u8)),
        ("bFlipShading", Box::new(false)),
        ("Outline", Box::new(0u8)),
        ("bSharpCorners", Box::new(true)),
        ("bSharpOutlines", Box::new(true)),
        ("bOverrideOutlineColor", Box::new(true)),
        ("bForeground", Box::new(false)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const GREEN: Rgba<u8> = Rgba([0, 255, 0, 255]);
    const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

    fn img(pixels: &[&[Rgba<u8>]]) -> RgbaImage {
        let h = pixels.len() as u32;
        let w = pixels[0].len() as u32;
        let mut img = RgbaImage::new(w, h);
        for (y, row) in pixels.iter().enumerate() {
            for (x, p) in row.iter().enumerate() {
                img.put_pixel(x as u32, y as u32, *p);
            }
        }
        img
    }

    /// Encode with defaults, asserting a single band results.
    fn text(i: &RgbaImage) -> String {
        let bands = encode_bands(i, &TextOptions::default()).unwrap();
        assert_eq!(bands.len(), 1);
        bands.into_iter().next().unwrap().text
    }

    #[test]
    fn single_opaque_pixel() {
        assert_eq!(text(&img(&[&[RED]])), "<color=\"FF0000\">██");
    }

    #[test]
    fn trailing_transparent_trimmed() {
        assert_eq!(text(&img(&[&[RED, CLEAR]])), "<color=\"FF0000\">██");
    }

    #[test]
    fn custom_empty_char_kept() {
        let opts = TextOptions {
            empty_char: '.',
            ..Default::default()
        };
        let bands = encode_bands(&img(&[&[RED, CLEAR]]), &opts).unwrap();
        assert_eq!(bands[0].text, "<color=\"FF0000\">██..");
    }

    #[test]
    fn color_run_spans_transparent_gap() {
        assert_eq!(
            text(&img(&[&[RED, CLEAR, RED]])),
            "<color=\"FF0000\">██  ██"
        );
    }

    #[test]
    fn color_run_spans_rows() {
        assert_eq!(text(&img(&[&[RED], &[RED]])), "<color=\"FF0000\">██\n██");
    }

    #[test]
    fn color_change_emits_new_tag() {
        assert_eq!(
            text(&img(&[&[RED, GREEN]])),
            "<color=\"FF0000\">██<color=\"00FF00\">██"
        );
    }

    #[test]
    fn alpha_threshold_boundary() {
        let below = Rgba([255, 0, 0, 127]);
        let at = Rgba([255, 0, 0, 128]);
        assert_eq!(text(&img(&[&[below, at]])), "  <color=\"FF0000\">██");
    }

    #[test]
    fn char_repeat_three() {
        let opts = TextOptions {
            char_repeat: 3,
            ..Default::default()
        };
        let bands = encode_bands(&img(&[&[RED]]), &opts).unwrap();
        assert_eq!(bands[0].text, "<color=\"FF0000\">███");
    }

    #[test]
    fn fully_transparent_rows_keep_line_breaks() {
        assert_eq!(text(&img(&[&[CLEAR], &[RED]])), "\n<color=\"FF0000\">██");
    }

    /// 20-wide row of per-pixel alternating colors = 20 × (16 + 2) = 360 chars.
    /// Band capacity: 360 + 361·(n−1) ≤ 10 000 ⇒ 27 rows. 60 rows ⇒ 27+27+6.
    #[test]
    fn bands_split_at_char_limit() {
        let mut i = RgbaImage::new(20, 60);
        for y in 0..60 {
            for x in 0..20 {
                i.put_pixel(x, y, if x % 2 == 0 { RED } else { GREEN });
            }
        }
        let bands = encode_bands(&i, &TextOptions::default()).unwrap();
        assert_eq!(
            bands.iter().map(|b| b.start_row).collect::<Vec<_>>(),
            vec![0, 27, 54]
        );
        assert_eq!(bands.iter().map(|b| b.rows).sum::<usize>(), 60);
        for b in &bands {
            assert!(b.chars <= MAX_COMPONENT_CHARS);
            assert_eq!(b.chars, b.text.chars().count());
            // start_row padding newlines first, then content with fresh color
            assert!(
                b.text[..b.start_row].chars().all(|c| c == '\n'),
                "band must start with start_row padding newlines"
            );
            assert!(
                b.text[b.start_row..].starts_with("<color=\""),
                "band must reset color state"
            );
        }
    }

    #[test]
    fn images_tile_in_both_axes() {
        // 72×72 alternating-color pixels: 3×3 tile grid (32+32+8 each axis)
        let mut i = RgbaImage::new(72, 72);
        for y in 0..72 {
            for x in 0..72 {
                i.put_pixel(x, y, if x % 2 == 0 { RED } else { GREEN });
            }
        }
        let tiles = encode_tiles(&i, &TextOptions::default()).unwrap();
        assert_eq!(tiles.len(), 9, "3x3 grid of {TILE_PX}px tiles");
        let expected_origins: Vec<(usize, usize)> = [0, 32, 64]
            .iter()
            .flat_map(|&r| [0usize, 32, 64].iter().map(move |&c| (c, r)))
            .collect();
        assert_eq!(
            tiles
                .iter()
                .map(|t| (t.start_col, t.start_row))
                .collect::<Vec<_>>(),
            expected_origins
        );
        for t in &tiles {
            assert!(t.bands.iter().all(|b| b.chars <= MAX_COMPONENT_CHARS));
            let rows_expected = if t.start_row == 64 { 8 } else { 32 };
            assert_eq!(t.bands.iter().map(|b| b.rows).sum::<usize>(), rows_expected);
        }

        // each tile's anchor cube sits AT its patch position scaled by the
        // rendered pitch, translated so all coordinates stay non-negative
        // (text right = world -Y ⇒ rightmost tile at y=0, image grows +Y;
        // brdb's negative-chunk encoding is unreliable in-game); z descends
        // with start_row from the top tile row (bottom row at z=1); extra
        // band cubes stack directly above their tile anchor
        let d = TextOptions::default();
        let pitch = d.line_world_height * d.pitch_scale;
        let max_span = (64.0 * pitch).round() as i32;
        let per_tile: Vec<(i32, i32, i32)> = tiles
            .iter()
            .map(|t| {
                (
                    max_span - (t.start_col as f32 * pitch).round() as i32,
                    1 + ((64 - t.start_row as i32) as f32 * pitch).round() as i32,
                    t.bands.len() as i32,
                )
            })
            .collect();
        let mut world = brdb::World::new();
        add_text_tiles(&mut world, tiles, &TextOptions::default());
        for (y, z_tile, n) in per_tile {
            let mut zs: Vec<i32> = world
                .bricks
                .iter()
                .filter(|b| {
                    b.position.y == y && b.position.z >= z_tile && b.position.z < z_tile + 2 * n
                })
                .map(|b| b.position.z)
                .collect();
            zs.sort_unstable();
            let expected: Vec<i32> = (0..n).map(|i| z_tile + 2 * i).collect();
            assert_eq!(zs, expected, "tile at y={y} anchors at z={z_tile}");
        }
        assert!(world.bricks.iter().all(|b| b.position.z >= 1));
        assert!(world.bricks.iter().all(|b| b.position.x == 0));
        // every coordinate must be non-negative (brdb negative-chunk bug)
        assert!(world.bricks.iter().all(|b| b.position.y >= 0));
    }

    #[test]
    fn fully_transparent_tiles_are_skipped() {
        // 64×32: left half opaque, right half fully transparent
        let mut i = RgbaImage::new(64, 32);
        for y in 0..32 {
            for x in 0..32 {
                i.put_pixel(x, y, RED);
            }
        }
        let tiles = encode_tiles(&i, &TextOptions::default()).unwrap();
        assert_eq!(tiles.len(), 1, "transparent tile must be skipped");
        assert_eq!((tiles[0].start_col, tiles[0].start_row), (0, 0));
    }

    #[test]
    fn row_too_wide_is_error() {
        let mut i = RgbaImage::new(600, 1);
        for x in 0..600 {
            i.put_pixel(x, 0, if x % 2 == 0 { RED } else { GREEN });
        }
        let err = encode_bands(&i, &TextOptions::default()).unwrap_err();
        assert!(err.contains("row 0"), "unexpected error: {err}");
    }

    #[test]
    fn font_metrics_solve() {
        // measured: 10 rows span 16.4 units, 10 cols span 8.2 units at LH 1
        let m = FontMetrics::from_measured(16.4, 8.2);
        let (lh, kerning) = m.solve(1.0, 2);
        // line advance 1.64/LH ⇒ LH ≈ 0.6098 for 1 unit rows
        assert!((lh - 1.0 / 1.64).abs() < 1e-6);
        // two glyphs at 0.82/LH each must fit 1 unit ⇒ kerning closes the gap
        assert!((kerning - (0.5 - 0.82 * lh)).abs() < 1e-6);
        // a measuring world builds, stays non-negative z, and actually
        // serializes (catches missing component registration)
        let world = build_measuring_world(&TextOptions::default());
        assert!(world.bricks.iter().all(|b| b.position.z >= 0));
        assert!(world.bricks.len() > 40, "block + rulers + sweeps present");
        world.to_brz_vec().expect("measuring world must encode to brz");
    }

    #[test]
    fn bricks_stack_one_per_band() {
        use brdb::{Position, World};
        let mut i = RgbaImage::new(20, 60);
        for y in 0..60 {
            for x in 0..20 {
                i.put_pixel(x, y, if x % 2 == 0 { RED } else { GREEN });
            }
        }
        let opts = TextOptions::default();
        let bands = encode_bands(&i, &opts).unwrap();
        let mut world = World::new();
        add_text_bricks(&mut world, bands, &opts);

        assert_eq!(world.bricks.len(), 3);
        // a single tile anchors at z=1; extra band cubes stack upward from
        // the anchor (their text shifts back down via the component Offset)
        assert_eq!(world.bricks[0].position, Position::new(0, 0, 1));
        assert_eq!(world.bricks[1].position, Position::new(0, 0, 3));
        assert_eq!(world.bricks[2].position, Position::new(0, 0, 5));
        assert_eq!(world.bricks[0].components.len(), 1);
        for b in &world.bricks {
            assert!(!b.visible, "anchor bricks must be invisible");
            assert!(
                !b.collision.player
                    && !b.collision.weapon
                    && !b.collision.interact
                    && !b.collision.tool
                    && !b.collision.physics,
                "anchor bricks must have no collision"
            );
        }
        assert!(
            world
                .global_data
                .external_asset_references
                .contains(&("BrickFontDescriptor".to_string(), "MonaspaceArgon".to_string()))
        );
    }
}
