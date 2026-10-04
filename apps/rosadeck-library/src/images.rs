//! Kitty graphics layer: real cover images at the terminal's native resolution.
//!
//! Half blocks are the fallback (one pixel per character column, which is why
//! covers looked blurry), but kitty can display the actual file scaled into a
//! cell rectangle — smooth, at full device resolution. Covers are therefore
//! *replaced* by transmitted images.
//!
//! Every detail here comes from the protocol spec (kitty 0.48), and each one was
//! a bug at some point:
//!
//! * `q=2` (quiet) on **every** command — otherwise the terminal answers on
//!   stdin (`ESC _Gi=1;OKESC \`) and those bytes reach the keyboard parser as
//!   bogus key events. That is what made the UI break on the one game that had
//!   artwork.
//! * `C=1` (no cursor movement) — after a placement the terminal moves the
//!   cursor by the rectangle size, which scrolls the screen (and the spec calls
//!   the position "undefined" when it leaves the scroll area).
//! * one image id per file **plus** a matching placement id, so moving a cover
//!   is `a=p` with the same ids: kitty replaces the placement in place, without
//!   a delete/re-place flicker and without a gap.
//! * `d=i` (lowercase) removes a placement but keeps the data, so coming back to
//!   a cover costs ~30 bytes instead of the whole file again.
//!
//! Terminals without the protocol get no bytes at all: the caller falls back to
//! half-block art.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Chunk size for base64 payloads (kitty accepts up to 4096 bytes per chunk).
const CHUNK: usize = 4096;
/// Files above this size are skipped rather than slowing every repaint.
pub const MAX_IMAGE_BYTES: u64 = 6 * 1024 * 1024;

/// One cover to show on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Image file (must exist and be within [`MAX_IMAGE_BYTES`]).
    pub path: PathBuf,
    /// Top-left cell (1-based on the wire).
    pub col: u16,
    /// Top-left row (1-based on the wire).
    pub row: u16,
    /// Columns the card draws on screen (the visible slice).
    pub cols: u16,
    /// Columns of the whole card, hidden part included: needed to translate
    /// the hidden columns into image pixels.
    pub card_w: u16,
    /// Height in cells.
    pub rows: u16,
    /// How this cover is drawn (crisp on top, recessed for the neighbours).
    pub treatment: crate::cover::Treatment,
    /// Backdrop the recession fades towards (part of the treatment).
    pub backdrop: crate::theme::Rgb,
    /// Columns of the card hidden behind the closer neighbour.
    pub hidden: usize,
    /// Which edge is visible (`true` = the left edge of the card).
    pub keep_left: bool,
    /// Native pixel size of the file, to translate the crop into kitty's
    /// source rectangle.
    pub image: (u32, u32),
}

/// Image id used for the startup probe; nothing else may use it.
const PROBE_ID: u32 = 4_294_000_000;

impl Plan {
    /// The rectangle, which identifies a placement.
    fn rect(&self) -> (u16, u16, u16, u16) {
        (self.col, self.row, self.cols, self.rows)
    }

    /// Stacking order: the selected cover is above its neighbours, so the
    /// middle of the shelf is the top layer of the image layer too.
    fn z(&self) -> i32 {
        if self.treatment == crate::cover::Treatment::Crisp { 1 } else { 0 }
    }

    /// What kitty is asked to hold for this plan: the file and the size it is
    /// transmitted at. A cover sent reduced for a neighbour is a *different*
    /// payload from the same cover sent native for the selection, so it gets a
    /// different id.
    fn key(&self) -> (PathBuf, (u32, u32)) {
        (self.path.clone(), self.wire_size())
    }

    /// Stable number for the treatment, so a converted cover is cached once.
    fn treatment_key(&self) -> u32 {
        match self.treatment {
            crate::cover::Treatment::Crisp => 0,
            crate::cover::Treatment::Recessed(depth) => 1000 + depth as u32,
        }
    }


    /// Size of the payload that will really be sent for this plan.
    ///
    /// The focused card goes out at its native size; a recessed one is reduced
    /// first. The crop rectangle has to be expressed in *these* pixels: measured
    /// on the wire, a neighbour's rectangle was computed from the 600x900 file
    /// while the payload was a fraction of it, so the crop fell outside the
    /// image, kitty drew the whole cover instead of its outer edge, and the card
    /// showed up squeezed rather than cropped.
    fn wire_size(&self) -> (u32, u32) {
        match self.treatment {
            crate::cover::Treatment::Crisp => self.image,
            crate::cover::Treatment::Recessed(depth) => {
                crate::cover::fit_size(self.image, crate::cover::transmit_max_side(depth))
            }
        }
    }

    /// Source rectangle (in image pixels) of the visible slice, or `None` for
    /// the whole card.
    ///
    /// The card is `card_w` columns wide and the payload is `wire_size().0`
    /// pixels wide, so one column is that many pixels. Only the hidden columns
    /// are cut.
    fn source_rect(&self) -> Option<(u32, u32)> {
        if self.hidden == 0 || self.card_w == 0 {
            return None;
        }
        let (payload_w, _) = self.wire_size();
        if payload_w == 0 {
            return None;
        }
        // One column of the *whole card* is `payload_w / card_w` pixels, and the
        // card is drawn at that scale: the visible slice is the rest.
        let per_column = (payload_w as f64 / self.card_w as f64).round() as u32;
        let hidden_px = (self.hidden as u32).saturating_mul(per_column);
        if hidden_px == 0 || hidden_px >= payload_w {
            return None;
        }
        let w = payload_w - hidden_px;
        let x = if self.keep_left { 0 } else { hidden_px };
        Some((x, w))
    }
}

/// True when the terminal can read our files itself (`t=f`).
///
/// Verified once at startup, never assumed: a 1x1 PNG is written into our own
/// state directory and the terminal is asked to load it from there. `OK` means
/// the covers can travel as *paths* (about 200 bytes on the wire) instead of
/// base64 blobs; anything else — no answer, `ENOENT`, a terminal that does not
/// speak the protocol — falls back to sending the bytes.
///
/// This matters because the focused card travels at its native size: streaming
/// a 600x900 PNG on every keypress is a megabyte of base64 per step, and the
/// frame cannot be presented until the terminal has swallowed all of it.
pub fn probe_local_files(state_dir: &std::path::Path) -> bool {
    use std::io::{Read as _, Write as _};
    // Only inside kitty: without it there is nobody to answer, and the reader
    // below would sit on stdin waiting, which costs the first keypress.
    if std::env::var("KITTY_WINDOW_ID").is_err() {
        return false;
    }
    let dir = state_dir.join("covers");
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let probe = dir.join(".probe.png");
    // 1x1 opaque PNG.
    const TINY: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
        0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
        0x42, 0x60, 0x82,
    ];
    if std::fs::write(&probe, TINY).is_err() {
        return false;
    }
    let mut out = std::io::stdout();
    if write!(out, "\x1b_Ga=q,t=f,f=100,i={PROBE_ID};{}\x1b\\", base64(probe.to_string_lossy().as_bytes())).is_err() {
        let _ = std::fs::remove_file(&probe);
        return false;
    }
    if out.flush().is_err() {
        let _ = std::fs::remove_file(&probe);
        return false;
    }
    // The reply arrives on stdin, which is exactly the channel the keyboard
    // uses. A dedicated reader with a deadline means a terminal that never
    // answers cannot block the app; worst case it eats one keypress at startup.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 256];
        if let Ok(n) = stdin.read(&mut buf) {
            let _ = tx.send(buf[..n].to_vec());
        }
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(bytes) => {
                // `ESC _Gi=<id>;OK ESC \` is the only answer that means the file
                // was read; anything with an error code does not.
                ok = bytes
                    .windows(2)
                    .any(|w| w == b";O")
                    && !bytes.windows(6).any(|w| w == b"ENOENT");
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = std::fs::remove_file(&probe);
    ok
}

/// True when the terminal can display images this way.
pub fn available(term: &str, kitty_window: bool, disabled: bool) -> bool {
    !disabled && (term.contains("kitty") || kitty_window)
}

/// What a [`Layer`] should do for one frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sync {
    /// Placements to remove (the cover left the band).
    pub hide: Vec<u32>,
    /// Covers to place, in order.
    pub place: Vec<Pending>,
}

/// A cover waiting to be placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// Kitty image id (one per file, stable for the layer's lifetime).
    pub id: u32,
    /// Where and how big.
    pub plan: Plan,
    /// The file has never been transmitted: send the bytes first.
    pub data_needed: bool,
}

/// Keeps track of what is on screen so frames stay cheap.
#[derive(Debug, Default)]
pub struct Layer {
    /// (File, transmitted size) -> image id.
    ///
    /// The size is part of the key on purpose. A neighbour travels reduced and
    /// the focused card at its native size; keying only by file meant a cover
    /// first seen as a neighbour kept that small payload when it became the
    /// selection — measured on the wire, the "focused" card was arriving at
    /// 112x168 instead of 600x900. One transmission per (file, size) keeps both.
    sent: HashMap<(PathBuf, (u32, u32)), u32>,
    /// The terminal proved it can read our files (see [`probe_local_files`]).
    local_files: bool,
    /// Where the PNG copies live, for covers whose file is not a PNG.
    cache_dir: Option<PathBuf>,
    /// Image id -> rectangle currently on screen.
    shown: HashMap<u32, (u16, u16, u16, u16)>,
    /// The screen no longer matches `shown` (after a resize): rebuild it.
    unsynced: bool,
    next_id: u32,
}

impl Layer {
    /// New empty layer (covers travel as bytes).
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Layer that sends covers as *paths* when the terminal can read them.
    ///
    /// `local_files` must come from [`probe_local_files`] and `state_dir` is
    /// where the PNG copies of non-PNG covers go.
    pub fn with_local_files(local_files: bool, state_dir: Option<&std::path::Path>) -> Self {
        Self { local_files, cache_dir: state_dir.map(|d| d.join("covers")), ..Self::default() }
    }

    /// Materialise the artwork a few plans will need, before anyone asks for it.
    ///
    /// Encoding a neighbour means decoding a 600x900 cover, reducing it, blurring
    /// it and writing a PNG: ~30 ms each, and a keypress needs two of them at
    /// once. Doing it while the app is idle means a keystroke only has to
    /// *place* images the terminal already holds.
    ///
    /// At most `budget` covers per call, and the caller only calls when no key is
    /// pending: the work moves out of the keypress path, it does not sneak into
    /// it.
    pub fn prepare(&mut self, plans: &[Plan], budget: usize) -> usize {
        if !self.local_files {
            return 0;
        }
        let todo: Vec<Plan> = plans
            .iter()
            .filter(|p| !self.sent.contains_key(&p.key()))
            .take(budget)
            .cloned()
            .collect();
        let mut done = 0;
        for plan in todo {
            if self.png_path(&plan).is_some() {
                done += 1;
            }
        }
        done
    }

    /// Send the artwork a few plans will need, *without* placing it.
    ///
    /// The reported symptom was precise: moving between games lagged, and the
    /// lag vanished once every cover had been seen once. So the cost is the
    /// *first* transmission of each cover, and it lands on the keystroke. This
    /// moves it off: while the app is idle, the artwork for the next position
    /// goes to the terminal, and the keystroke only has to place images it
    /// already holds.
    ///
    /// Ids are assigned exactly as [`Layer::diff`] would, so a later frame finds
    /// them known and does not send them again.
    pub fn prefetch<W: Write>(&mut self, out: &mut W, plans: &[Plan], budget: usize) -> std::io::Result<usize> {
        let mut sent = 0;
        for (i, plan) in plans.iter().enumerate() {
            if sent >= budget {
                break;
            }
            let key = plan.key();
            if self.sent.contains_key(&key) {
                continue;
            }
            // Same id arithmetic as `diff`: images are numbered in plan order.
            let id = self.next_id + 1 + i as u32;
            let sent_now = match self.png_path(plan) {
                Some(path) if self.local_files => transmit_path(out, &path, id, plan).is_ok(),
                _ => match read(plan) {
                    Some(data) => transmit(out, &data, id, plan).is_ok(),
                    None => false,
                },
            };
            if sent_now {
                self.sent.insert(key, id);
                self.next_id = self.next_id.max(id);
                sent += 1;
            }
        }
        Ok(sent)
    }

    /// A PNG file on disk that kitty can read for this plan, if there is one.
    ///
    /// A focused card whose file already *is* a PNG is used as it is: nothing
    /// to encode, and the terminal reads it straight from disk. Anything else —
    /// a neighbour (reduced and blurred, so it does not exist as a file yet) or
    /// a cover in another format — is written once into the cache directory and
    /// reused for the rest of the session.
    ///
    /// It has to be the plan's *actual* payload: returning the original file for
    /// a recessed plan sent the crisp full-size cover, and the neighbour lost
    /// its blur.
    fn png_path(&self, plan: &Plan) -> Option<PathBuf> {
        use std::io::Read as _;
        // The focused card is the one that really pays: it travels at native
        // size, so a path saves hundreds of kilobytes. Neighbours are encoded
        // either way; putting them on disk is what keeps that encoding *out* of
        // the keypress (see [`Self::prepare`]).
        if !self.local_files {
            return None;
        }
        if matches!(plan.treatment, crate::cover::Treatment::Crisp) {
            let is_png = std::fs::File::open(&plan.path)
                .map(|mut f| {
                    let mut head = [0u8; 8];
                    f.read_exact(&mut head).is_ok() && head == PNG_MAGIC
                })
                .unwrap_or(false);
            if is_png {
                return Some(plan.path.clone());
            }
        }
        let dir = self.cache_dir.as_ref()?;
        if std::fs::create_dir_all(dir).is_err() {
            return None;
        }
        let stem = plan.path.file_stem()?.to_string_lossy().replace(['/', ' '], "_");
        let copy = dir.join(format!("{stem}-{}.png", plan.treatment_key()));
        if copy.is_file() {
            return Some(copy);
        }
        // Write to a temporary name first: kitty must never see a half-written
        // PNG, and a crashed run must not leave one behind.
        let tmp = copy.with_extension("tmp");
        let data = read(plan)?;
        if std::fs::write(&tmp, &data).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        if std::fs::rename(&tmp, &copy).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        Some(copy)
    }

    /// The screen no longer matches what we think is placed (a resize wipes
    /// everything). The next [`diff`] hides and re-places from scratch, so a
    /// cover that disappears can never be left stuck on the screen.
    pub fn invalidate(&mut self) {
        self.unsynced = true;
    }

    /// Delete every placement **and** forget the transmitted files (after a
    /// rescan: the covers on disk may have changed).
    ///
    /// Takes the writer on purpose: forgetting ids without deleting their
    /// placements would leave images stuck on the screen forever.
    pub fn forget<W: Write>(&mut self, out: &mut W) -> std::io::Result<()> {
        self.clear(out)?;
        self.sent.clear();
        Ok(())
    }

    /// Files whose bytes kitty is holding.
    pub fn transmitted(&self) -> usize {
        self.sent.len()
    }

    /// Compute what must change for `wanted`. Pure: safe to inspect.
    pub fn diff(&self, wanted: &[Plan]) -> Sync {
        // A cover that left the band entirely: hide its placement (keeping the
        // data). A cover that only *moved* is kept: `a=p` with the same image and
        // placement ids replaces it in place, without a delete/re-place flicker.
        // After `invalidate` the screen is rebuilt wholesale, so a cover that
        // vanished (a rescan) can never be left stuck on screen.
        let mut hide: Vec<u32> = self.shown.keys().copied().collect();
        if !self.unsynced {
            hide.retain(|id| !wanted.iter().any(|w| self.sent.get(&w.key()) == Some(id)));
        }
        hide.sort_unstable();
        let mut place = Vec::new();
        for plan in wanted {
            let known = self.sent.get(&plan.key()).copied();
            let id = match known {
                Some(id) => id,
                None => self.next_id + 1 + place.len() as u32,
            };
            // While unsynced everything was just hidden, so it must be placed again.
            let already = !self.unsynced && known.is_some_and(|id| self.shown.get(&id) == Some(&plan.rect()));
            if !already {
                place.push(Pending { id, plan: plan.clone(), data_needed: known.is_none() });
            }
        }
        Sync { hide, place }
    }

    /// Emit the byte stream for `sync`: hide what left, then place what shows.
    ///
    /// Returns the ids that were really placed, so unreadable files are never
    /// remembered.
    pub fn write<W: Write>(&self, out: &mut W, sync: &Sync) -> std::io::Result<Vec<u32>> {
        for id in &sync.hide {
            // Lowercase `i` removes the placement but keeps the data, so the
            // cover can come back with `a=p` and no payload.
            out.write_all(format!("\x1b_Ga=d,d=i,i={id},p={id},q=2\x1b\\").as_bytes())?;
        }
        let mut placed = Vec::new();
        for pending in &sync.place {
            if pending.data_needed {
                // With a terminal that can read our files the payload is a
                // path, not a megabyte of base64. Only the probe can say yes.
                match self.png_path(&pending.plan) {
                    Some(path) if self.local_files => transmit_path(out, &path, pending.id, &pending.plan)?,
                    _ => {
                        let Some(data) = read(&pending.plan) else { continue };
                        transmit(out, &data, pending.id, &pending.plan)?;
                    }
                }
            }
            // Park the cursor on the cell: the placement is relative to it.
            out.write_all(format!("\x1b[{};{}H", pending.plan.row, pending.plan.col).as_bytes())?;
            let src = match pending.plan.source_rect() {
                Some((x, w)) => format!("x={x},w={w},"),
                None => String::new(),
            };
            out.write_all(
                format!(
                    "\x1b_Ga=p,i={},p={},q=2,C=1,z={},{src}c={},r={}\x1b\\",
                    pending.id, pending.id, pending.plan.z(), pending.plan.cols, pending.plan.rows
                )
                .as_bytes(),
            )?;
            placed.push(pending.id);
        }
        Ok(placed)
    }

    /// Record the state after a successful [`write`].
    pub fn commit(&mut self, sync: &Sync, placed: &[u32]) {
        for id in &sync.hide {
            self.shown.remove(id);
        }
        self.unsynced = false;
        for pending in &sync.place {
            if placed.contains(&pending.id) {
                self.shown.insert(pending.id, pending.plan.rect());
                self.next_id = self.next_id.max(pending.id);
                if pending.data_needed {
                    self.sent.insert(pending.plan.key(), pending.id);
                }
            }
        }
    }

    /// Kitty has forgotten every image, without us asking: the painter emits
    /// `\x1b[2J` on a full repaint and, per the graphics spec, the clear screen
    /// code "should also clear all images" — not only the placements, the data
    /// too.
    ///
    /// So after a `[2J` a bare `a=p` gets `ENOENT` and the covers stay dark.
    /// Nothing has to be deleted here (kitty already dropped them); the point is
    /// to stop believing we still have them, so the next frame re-transmits.
    /// With `t=f` that is just the path again (13 KiB for a shelf of seven).
    pub fn data_lost(&mut self) {
        self.shown.clear();
        self.sent.clear();
    }

    /// Delete every placement (on exit). Uppercase `A` also frees the data.
    pub fn clear<W: Write>(&mut self, out: &mut W) -> std::io::Result<()> {
        if self.shown.is_empty() {
            return Ok(());
        }
        // q=2 here too: every graphics command we send stays quiet.
        out.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\")?;
        self.shown.clear();
        Ok(())
    }
}

/// Transmit one image's bytes (kitty keeps them until we delete the id).
///
/// Hand the terminal a *path* instead of the pixels (`t=f`).
///
/// About 200 bytes on the wire whatever the cover weighs, and the terminal
/// reads and decodes it itself. `q=2` still, so nothing comes back on stdin, and
/// `i=` so the image is stored and can be re-placed later without resending.
fn transmit_path<W: Write>(out: &mut W, path: &Path, id: u32, plan: &Plan) -> std::io::Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    out.write_all(
        format!(
            "\x1b_Ga=t,f=100,t=f,q=2,i={id},z={},c={},r={};{}\x1b\\",
            plan.z(),
            plan.cols,
            plan.rows,
            base64(absolute.to_string_lossy().as_bytes())
        )
        .as_bytes(),
    )?;
    Ok(())
}

/// `a=t` sends the base64 payload in chunks; the placement comes separately so a
/// later frame can re-place the same artwork without sending the file again.
fn transmit<W: Write>(out: &mut W, data: &[u8], id: u32, plan: &Plan) -> std::io::Result<()> {
    let b64 = base64(data);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        // a=t transmit, f=100 PNG payload, q=2 no reply, i=id, c/r the rectangle,
        // z=0 *above* the text: the cover area is blank by construction, and a
        // repainted space must never hide the artwork (z<0 did exactly that —
        // any row diff over the band erased the cover).
        let head = format!("\x1b_Ga=t,f=100,q=2,i={id},z={},c={},r={},m={more};", plan.z(), plan.cols, plan.rows);
        out.write_all(head.as_bytes())?;
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// PNG file signature (`\x89PNG\r\n\x1a\n`).
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Read a cover as **PNG** bytes, honouring [`MAX_IMAGE_BYTES`].
///
/// `f=100` tells kitty the payload is a PNG. A JPEG (or WebP, AVIF, …) sent as
/// `f=100` is rejected silently — `q=2` means we never see the error — and the
/// cover simply never appears. So anything that is not already a PNG is decoded
/// and re-encoded here, once per file.
fn read(plan: &Plan) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(&plan.path).ok()?;
    if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let raw = std::fs::read(&plan.path).ok()?;
    // Recessed covers are the neighbours: they must reach kitty already
    // blurred and desaturated, or they would arrive as crisp artwork and the
    // shelf would lose its depth. Same code path as the half-block renderer.
    if let crate::cover::Treatment::Recessed(depth) = plan.treatment {
        // Reduce first, then recede: the blur is meant to be seen at cover size,
        // and this is the difference between milliseconds and seconds. The size
        // comes from the depth, so the further back the card, the fewer pixels
        // it gets.
        let decoded = image::load_from_memory(&raw).ok()?.to_rgb8();
        let mut small = crate::cover::fit_for_transmit(&decoded, crate::cover::transmit_max_side(depth));
        crate::cover::treat(&mut small, plan.treatment, plan.backdrop);
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(small)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        return png.starts_with(PNG_MAGIC).then_some(png);
    }
    if raw.starts_with(PNG_MAGIC) {
        return Some(raw); // already PNG: send the bytes untouched
    }
    let decoded = image::load_from_memory(&raw).ok()?.to_rgba8();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(decoded)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    png.starts_with(PNG_MAGIC).then_some(png)
}

/// Minimal base64 encoder (same alphabet kitty expects).
fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for w in data.chunks(3) {
        let n = (w[0] as u32) << 16 | (*w.get(1).unwrap_or(&0) as u32) << 8 | (*w.get(2).unwrap_or(&0) as u32);
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if w.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if w.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Convenience for callers and tests: the plan for one cell.
#[cfg(test)]
pub fn plan_for(
    path: &Path,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    treatment: crate::cover::Treatment,
    backdrop: crate::theme::Rgb,
) -> Plan {
    let image = image_dimensions(path);
    Plan { path: path.to_path_buf(), col, row, cols, card_w: cols, rows, treatment, backdrop, hidden: 0, keep_left: true, image }
}

/// A neighbour's plan: the card is cropped on its hidden edge.
///
/// kitty scales the *source rectangle* (`x`/`w`) into the cell rectangle, so a
/// neighbour is cropped rather than squeezed and keeps the artwork's scale.
pub fn plan_for_cropped(
    path: &Path,
    col: u16,
    row: u16,
    card_w: u16,
    rows: u16,
    crop: crate::cover::Crop,
    treatment: crate::cover::Treatment,
    backdrop: crate::theme::Rgb,
) -> Plan {
    let image = image_dimensions(path);
    // `card_w` is the whole card; `crop` says how many of those columns the
    // card actually shows (a neighbour's inner edge hides behind the focus),
    // and the rest is `hidden`. `Crop::FULL` keeps everything.
    let full = card_w as usize;
    let cols = match crop.kept() {
        0 => full,
        kept => kept.min(full),
    };
    Plan {
        path: path.to_path_buf(),
        col,
        row,
        // The card is drawn at its *full* scale, but only its visible slice
        // takes cells: `cols` columns, cropped out of `card_w`.
        cols: cols as u16,
        card_w,
        rows,
        treatment,
        backdrop,
        hidden: full - cols,
        keep_left: crop.keep_left > 0,
        image,
    }
}

/// Native size of an image, read from its header only.
fn image_dimensions(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKDROP: crate::theme::Rgb = (18, 18, 24);

    fn p(name: &str, col: u16) -> Plan {
        plan_for(Path::new(name), col, 6, 10, 12, crate::cover::Treatment::Crisp, BACKDROP)
    }

    /// The same cell, but a neighbour: smaller, recessed and below the top.
    fn recessed(name: &str, col: u16, rows: u16) -> Plan {
        plan_for(Path::new(name), col, 8, 10, rows, crate::cover::Treatment::Recessed(50), BACKDROP)
    }

    /// A PNG big enough to need several kitty chunks (noise, so it does not
    /// compress down to nothing).
    fn write_noisy_png(path: &Path, w: u32, h: u32) {
        let mut img = image::RgbImage::new(w, h);
        let mut seed = 0x2545_f491u32;
        for px in img.pixels_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            px.0 = [seed as u8, (seed >> 8) as u8, (seed >> 16) as u8];
        }
        img.save(path).unwrap();
    }

    /// Write a real, loadable PNG. The old fixtures wrote `b"x"`, which only
    /// worked because the payload was sent untouched; now a non-image is
    /// (correctly) rejected before it reaches the terminal.
    fn write_png(path: &Path) {
        image::RgbImage::from_pixel(12, 18, image::Rgb([200, 40, 90])).save(path).unwrap();
    }

    /// A neighbour's plan shows its *visible* columns on the outer edge and
    /// crops the rest out of the image, with the two counts never swapped.
    ///
    /// This is the bug that made the fan look broken: `Crop::hidden()` returned
    /// the number of columns it *kept*, so the plan asked kitty for
    /// `full - kept` cells and cropped `kept` pixels away, and the neighbour's
    /// artwork landed on top of the focused card.
    #[test]
    fn a_neighbours_plan_crops_the_inner_edge_and_keeps_its_visible_columns() {
        let dir = std::env::temp_dir().join("rosadeck-images-crop");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover.png");
        image::RgbImage::from_pixel(240, 360, image::Rgb([200, 40, 90])).save(&path).unwrap();
        // The focus: the whole card, nothing cropped.
        let focus = recessed("cover.png", 40, 22);
        assert_eq!(focus.cols, focus.card_w, "nothing hidden on the focus");
        assert_eq!(focus.hidden, 0);
        assert!(focus.source_rect().is_none(), "an uncropped card sends the whole image");

        // A 24-column card showing its 11 outer columns: 13 hide behind the focus.
        for (crop, outer_is_left) in [(crate::cover::Crop::left(11), true), (crate::cover::Crop::right(11), false)] {
            let plan = plan_for_cropped(&path, 5, 8, 24, 22, crop, crate::cover::Treatment::Recessed(50), BACKDROP);
            assert_eq!(plan.cols, 11, "the visible slice takes cells: {plan:?}");
            assert_eq!(plan.card_w, 24, "but the card keeps its full scale: {plan:?}");
            assert_eq!(plan.hidden, 13, "the rest hides behind the closer card: {plan:?}");
            assert_eq!(plan.keep_left, outer_is_left, "{plan:?}");
            // The rectangle is in the pixels that actually go on the wire, which
            // for a recessed cover is the *reduced* image: computed against the
            // 240x360 file it fell outside the 116x174 payload, kitty drew the
            // whole cover and the card showed up squeezed.
            let (payload_w, _) = plan.wire_size();
            assert!(payload_w < plan.image.0, "a recessed cover travels reduced: {plan:?}");
            let per_column = payload_w / plan.card_w as u32;
            let hidden_px = 13 * per_column;
            let (x, w) = plan.source_rect().expect("a cropped card crops the image").clone();
            assert_eq!(w, payload_w - hidden_px, "the kept part of the image: {plan:?}");
            assert!(
                x + w <= payload_w,
                "the crop stays inside the payload that is actually sent: {plan:?}"
            );
            if outer_is_left {
                assert_eq!(x, 0, "the outer edge kept is the left one: {plan:?}");
            } else {
                assert_eq!(x, hidden_px, "the outer edge kept is the right one: {plan:?}");
                assert_eq!(x + w, payload_w, "and it runs to the far edge: {plan:?}");
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detection_requires_a_capable_terminal() {
        assert!(available("xterm-kitty", false, false));
        assert!(available("xterm-256color", true, false), "KITTY_WINDOW_ID also counts");
        assert!(!available("xterm-256color", false, false));
        assert!(!available("foot", false, false));
        assert!(!available("xterm-kitty", false, true), "--no-images wins");
    }

    #[test]
    fn every_command_is_quiet_and_never_moves_the_cursor() {
        // q=2 missing means the terminal answers on stdin and crossterm reads
        // that as key presses; C=1 missing means the cursor jumps after the
        // placement. Both broke the UI on the game with artwork.
        let dir = std::env::temp_dir().join("rosadeck-images-quiet");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.png");
        // Big enough to need several chunks, and a real image so it is sent.
        image::RgbImage::from_pixel(120, 180, image::Rgb([7, 8, 9])).save(&path).unwrap();
        let mut l = Layer::new();
        let wanted = vec![plan_for(&path, 3, 6, 21, 30, crate::cover::Treatment::Crisp, BACKDROP)];
        let sync = l.diff(&wanted);
        let mut buf = Vec::new();
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        let text = String::from_utf8_lossy(&buf).into_owned();
        for cmd in text.split("\x1b_G").skip(1) {
            let head = cmd.split(&[';', '\x1b'][..]).next().unwrap_or("");
            assert!(head.contains("q=2"), "comando sin q=2 (respondería por stdin): {head}");
        }
        assert!(text.contains("C=1"), "el cursor no debe moverse al colocar");
        // Now pan it: only a placement, still quiet, no payload.
        let moved = vec![plan_for(&path, 30, 6, 21, 30, crate::cover::Treatment::Crisp, BACKDROP)];
        let sync = l.diff(&moved);
        let mut buf = Vec::new();
        let again = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &again);
        let text = String::from_utf8_lossy(&buf).into_owned();
        assert!(text.contains("a=p") && !text.contains("m=1;"), "solo colocación: {text:?}");
        assert!(text.contains("C=1") && text.contains("q=2"));
        assert!(buf.len() < 120, "recolocar son {} bytes", buf.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn panning_replaces_the_placement_without_delete_or_payload() {
        let dir = std::env::temp_dir().join("rosadeck-images-move");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.png");
        image::RgbImage::from_pixel(64, 96, image::Rgb([2, 8, 9])).save(&path).unwrap();
        let mut l = Layer::new();
        let first = vec![plan_for(&path, 3, 6, 21, 30, crate::cover::Treatment::Crisp, BACKDROP)];
        let s1 = l.diff(&first);
        let mut buf = Vec::new();
        let w = l.write(&mut buf, &s1).unwrap();
        l.commit(&s1, &w);
        let moved = vec![plan_for(&path, 40, 6, 21, 30, crate::cover::Treatment::Crisp, BACKDROP)];
        let s2 = l.diff(&moved);
        assert!(s2.hide.is_empty(), "the placement is replaced, not deleted");
        assert_eq!(s2.place.len(), 1);
        assert!(!s2.place[0].data_needed, "same file, no payload");
        assert_eq!(s2.place[0].id, w[0], "same image and placement id");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn leaving_the_band_hides_the_placement_but_keeps_the_data() {
        let dir = std::env::temp_dir().join("rosadeck-images-hide");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.png");
        image::RgbImage::from_pixel(64, 96, image::Rgb([3, 8, 9])).save(&path).unwrap();
        let mut l = Layer::new();
        let first = vec![plan_for(&path, 3, 6, 21, 30, crate::cover::Treatment::Crisp, BACKDROP)];
        let s1 = l.diff(&first);
        let mut buf = Vec::new();
        let w = l.write(&mut buf, &s1).unwrap();
        l.commit(&s1, &w);
        let s2 = l.diff(&[]);
        let w = w.clone();
        assert_eq!(s2.hide, w.clone(), "hide the placement");
        let mut buf = Vec::new();
        l.write(&mut buf, &s2).unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        assert!(text.contains("d=i"), "lowercase keeps the data: {text:?}");
        assert!(text.contains(&format!("i={},p={}", w[0], w[0])));
        l.commit(&s2, &[]);
        // Coming back re-places without the payload.
        let back = l.diff(&first);
        assert!(!back.place[0].data_needed);
        assert_eq!(l.transmitted(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Prefetching while idle must not change what a later frame sends.
    ///
    /// The ids are assigned the way `diff` would assign them, so the frame that
    /// needs the artwork only *places* it. If the ids drifted, the same cover
    /// would be transmitted twice or placed with an image the terminal never
    /// got.
    #[test]
    fn prefetching_leaves_the_next_frame_with_only_placements() {
        let dir = std::env::temp_dir().join("rosadeck-images-prefetch");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover.png");
        image::RgbImage::from_pixel(600, 900, image::Rgb([90, 30, 200])).save(&path).unwrap();
        let focus = plan_for(&path, 40, 6, 45, 30, crate::cover::Treatment::Crisp, BACKDROP);
        let neighbour = plan_for_cropped(
            &path,
            90,
            8,
            24,
            22,
            crate::cover::Crop::right(11),
            crate::cover::Treatment::Recessed(46),
            BACKDROP,
        );

        // Bytes mode: the prefetch has to send the payload itself.
        let mut l = Layer::new();
        let mut buf = Vec::new();
        let sent = l.prefetch(&mut buf, &[neighbour.clone(), focus.clone()], 3).unwrap();
        assert_eq!(sent, 2, "both covers transmitted while idle");
        assert!(buf.windows(9).any(|w| w == b"a=t,f=100"), "the payload went out: {}", buf.len());

        // Now the frame that needs them: no payload, only placements.
        let want = vec![neighbour, focus];
        let sync = l.diff(&want);
        assert!(
            sync.place.iter().all(|p| !p.data_needed),
            "the prefetched covers must not be sent again: {sync:?}"
        );
        let mut buf = Vec::new();
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        assert!(!buf.windows(9).any(|w| w == b"a=t,f=100"), "no transmission, only placements");
        assert_eq!(buf.windows(4).filter(|w| *w == b"a=p,").count(), 2, "two placements");

        // And a cover that was never prefetched is still transmitted normally.
        let other = dir.join("other.png");
        image::RgbImage::from_pixel(60, 90, image::Rgb([10, 200, 90])).save(&other).unwrap();
        let fresh = plan_for(&other, 10, 6, 20, 30, crate::cover::Treatment::Crisp, BACKDROP);
        let sync = l.diff(&[fresh]);
        assert!(sync.place[0].data_needed, "an unseen cover still needs its bytes");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A cover first seen as a neighbour (reduced) must be sent again at native
    /// size when it becomes the selection: keying the transmitted data by file
    /// alone kept the small payload and the "focused" card arrived at 112x168.
    #[test]
    fn a_cover_that_becomes_the_focus_is_sent_at_its_native_size() {
        let dir = std::env::temp_dir().join("rosadeck-images-focus-size");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover.png");
        image::RgbImage::from_pixel(600, 900, image::Rgb([30, 90, 200])).save(&path).unwrap();

        let mut l = Layer::new();
        let neighbour = plan_for_cropped(
            &path,
            5,
            8,
            24,
            22,
            crate::cover::Crop::right(11),
            crate::cover::Treatment::Recessed(46),
            BACKDROP,
        );
        let mut buf = Vec::new();
        let first = l.diff(&[neighbour.clone()]);
        let written = l.write(&mut buf, &first).unwrap();
        l.commit(&first, &written);
        assert_eq!(l.transmitted(), 1);

        // Same file, now the selection: a different payload, so it goes again.
        let focus = plan_for(&path, 40, 6, 45, 30, crate::cover::Treatment::Crisp, BACKDROP);
        let second = l.diff(&[focus]);
        assert!(
            second.place.first().map(|p| p.data_needed).unwrap_or(false),
            "the focused card must be re-sent at its native size, not reused reduced"
        );
        assert_ne!(second.place[0].id, written[0], "and it needs its own image id");
        let mut buf = Vec::new();
        let written2 = l.write(&mut buf, &second).unwrap();
        l.commit(&second, &written2);
        assert_eq!(l.transmitted(), 2, "two payloads for one file");

        // And going back to the neighbour does not send a third time.
        let third = l.diff(&[neighbour]);
        assert!(
            !third.place.first().map(|p| p.data_needed).unwrap_or(true),
            "the reduced payload is still known"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn first_frame_places_everything_and_sends_nothing_else() {
        let mut l = Layer::new();
        let wanted = vec![p("a.png", 1), p("b.png", 20)];
        let sync = l.diff(&wanted);
        assert!(sync.hide.is_empty());
        assert_eq!(sync.place.len(), 2);
        assert_eq!(sync.place[0].id, 1);
        assert_eq!(sync.place[1].id, 2);
        assert!(sync.place.iter().all(|p| p.data_needed));
        l.commit(&sync, &[1, 2]);
        // Same frames again: nothing to do.
        assert_eq!(l.diff(&wanted), Sync::default());
        assert_eq!(l.transmitted(), 2);
    }

    #[test]
    fn resize_replaces_every_placement() {
        let mut l = Layer::new();
        let before = vec![p("a.png", 1)];
        let sync = l.diff(&before);
        l.commit(&sync, &[1]);
        // A resize changes the rectangle: the placement is replaced in place
        // (no delete, no payload), it is not hidden and re-sent.
        let after = vec![Plan { cols: 20, rows: 30, ..p("a.png", 1) }];
        let sync = l.diff(&after);
        assert!(sync.hide.is_empty(), "moved, not hidden: {sync:?}");
        assert_eq!(sync.place.len(), 1);
        assert_eq!(sync.place[0].id, 1, "same image id");
        assert!(!sync.place[0].data_needed, "the file is already in kitty");
        assert_eq!(sync.place[0].plan.cols, 20);
        // And it is hidden only when the cover leaves the band.
        assert_eq!(l.diff(&[]).hide, vec![1]);
    }

    /// The two ways to take the covers off the screen are not the same thing,
    /// and mixing them up is invisible in the app: the bytes it sends are always
    /// "correct", but kitty only keeps the data if it was told to.
    ///
    /// * `a=d,d=i` — placement gone, **data kept**: how a band is hidden for a
    ///   moment (the `d` window does it with `diff(&[])`), so coming back is a
    ///   plain `a=p`.
    /// * `a=d,d=A` — placement **and data** gone: only for leaving the alternate
    ///   screen. Used as a temporary hide, the covers never come back, because
    ///   the app still believes kitty holds the bytes (v26).
    #[test]
    fn hiding_the_band_and_leaving_the_screen_are_different_commands() {
        let dir = std::env::temp_dir().join("rosadeck-images-clear");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover.png");
        write_png(&path);
        let focus = plan_for(&path, 40, 6, 45, 30, crate::cover::Treatment::Crisp, BACKDROP);

        let mut l = Layer::new();
        let mut buf = Vec::new();
        let sync = l.diff(&[focus.clone()]);
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        assert_eq!(written.len(), 1, "la portada está en pantalla");

        // Esconder: el diff de una lista vacía.
        let mut buf = Vec::new();
        let sync = l.diff(&[]);
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        let text = String::from_utf8_lossy(&buf).into_owned();
        assert!(text.contains("a=d,d=i,"), "colocación fuera: {text:?}");
        assert!(!text.contains("d=A"), "el dato tiene que sobrevivir: {text:?}");

        // Volver: una colocación y ningún payload.
        let mut buf = Vec::new();
        let sync = l.diff(&[focus]);
        assert!(!sync.place[0].data_needed, "y sin reenviar: {sync:?}");
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        assert_eq!(written.len(), 1);
        assert!(!buf.windows(9).any(|w| w == b"a=t,f=100"), "sin payload: {buf:?}");

        // Salir: aquí sí se libera todo.
        let mut buf = Vec::new();
        l.clear(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("a=d,d=A"), "{buf:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A full repaint is `\x1b[2J`, and the graphics spec says the clear screen
    /// code "should also clear all images" — the data, not just the placements.
    /// So after one, a bare `a=p` gets `ENOENT` and the shelf goes dark until the
    /// covers are sent again. Measured against a real kitty (v26).
    #[test]
    fn a_full_repaint_makes_the_covers_be_sent_again() {
        let dir = std::env::temp_dir().join("rosadeck-images-2j");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cover.png");
        write_png(&path);
        let focus = plan_for(&path, 40, 6, 45, 30, crate::cover::Treatment::Crisp, BACKDROP);

        let mut l = Layer::new();
        let mut buf = Vec::new();
        let sync = l.diff(&[focus.clone()]);
        let w = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &w);
        assert_eq!(l.transmitted(), 1, "la portada se envió una vez");

        // El pintor va a emitir `[2J` (por eso `data_lost`), y kitty se queda
        // sin nada: el siguiente `a=p` respondería ENOENT.
        l.data_lost();

        let mut buf: Vec<u8> = Vec::new();
        let sync = l.diff(&[focus.clone()]);
        assert!(sync.place[0].data_needed, "tras un [2J] hay que reenviar la portada: {sync:?}");
        let w = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &w);
        assert!(buf.windows(9).any(|win| win == b"a=t,f=100"), "y se envía de verdad: {buf:?}");
        assert_eq!(l.transmitted(), 1, "y el id vuelve a ser el mismo");

        // Y esconder *sin* repintar (el diff de una lista vacía) no obliga a
        // reenviar: `d=i` conserva el dato.
        let mut buf = Vec::new();
        let sync = l.diff(&[]);
        let w = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &w);
        let sync = l.diff(&[focus]);
        assert!(!sync.place[0].data_needed, "esconder no obliga a reenviar: {sync:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn invalidate_keeps_files_but_forgets_placements() {
        let dir = std::env::temp_dir().join("rosadeck-images-invalidate");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        write_png(&file);
        let mut l = Layer::new();
        let wanted = vec![plan_for(&file, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)];
        let sync = l.diff(&wanted);
        let mut buf = Vec::new();
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        assert!(l.diff(&wanted).place.is_empty());
        l.invalidate();
        let rebuilt = l.diff(&wanted);
        assert_eq!(rebuilt.hide, vec![written[0]], "the stale placement is hidden");
        assert_eq!(rebuilt.place.len(), 1, "and placed again");
        assert!(!rebuilt.place[0].data_needed, "but not re-sent");
        let mut buf = Vec::new();
        let again = l.write(&mut buf, &rebuilt).unwrap();
        l.commit(&rebuilt, &again);
        let mut sink = Vec::new();
        l.forget(&mut sink).unwrap();
        assert_eq!(l.transmitted(), 0);
        assert!(l.diff(&wanted).place[0].data_needed, "after forget, re-transmit");
        // Forgetting deletes what was on screen, so nothing is left behind.
        assert!(String::from_utf8_lossy(&sink).contains("d=A"), "{}", String::from_utf8_lossy(&sink));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreadable_or_oversized_files_are_skipped() {
        let dir = std::env::temp_dir().join("rosadeck-images-missing");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read(&plan_for(&dir.join("nope.png"), 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)).is_none());
        let big = dir.join("big.png");
        std::fs::write(&big, vec![0u8; (MAX_IMAGE_BYTES + 1) as usize]).unwrap();
        assert!(read(&plan_for(&big, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)).is_none(), "too big to send");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transmission_is_well_formed() {
        let dir = std::env::temp_dir().join("rosadeck-images-transmit");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.png");
        write_noisy_png(&path, 300, 450);
        let mut l = Layer::new();
        let plan = plan_for(&path, 3, 6, 21, 30, crate::cover::Treatment::Crisp, BACKDROP);
        let sync = l.diff(&[plan.clone()]);
        let mut buf = Vec::new();
        let written = l.write(&mut buf, &sync).unwrap();
        l.commit(&sync, &written);
        assert_eq!(written, vec![1]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        assert!(text.contains("a=t,"), "transmit: {text:.60}");
        assert!(text.contains("z=1"), "the selected cover is the top layer");
        assert!(!text.contains("z=-"), "no negative z-index: those are erased by text");
        assert_eq!(text.matches("m=0;").count(), 1, "exactly one final chunk");
        // Many chunks (4096 base64 chars each) and every one of them but the
        // last must say m=1: a broken continuation drops the whole image.
        let chunks = text.matches("m=").count();
        assert!(chunks > 2, "a big cover is still split into chunks: {chunks}");
        assert_eq!(text.matches("m=1;").count(), chunks - 1, "only the last chunk is final");
        assert!(text.ends_with("c=21,r=30\x1b\\"), "then the placement");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_emits_one_delete_all_and_is_idempotent() {
        let dir = std::env::temp_dir().join("rosadeck-images-clear");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.png");
        write_png(&path);
        let mut l = Layer::new();
        let wanted = vec![plan_for(&path, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)];
        let sync = l.diff(&wanted);
        let mut skip = Vec::new();
        let w = l.write(&mut skip, &sync).unwrap();
        l.commit(&sync, &w);
        let mut buf = Vec::new();
        l.clear(&mut buf).unwrap();
        assert_eq!(buf, b"\x1b_Ga=d,d=A,q=2\x1b\\");
        let mut again = Vec::new();
        l.clear(&mut again).unwrap();
        assert!(again.is_empty(), "nothing left to delete");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_cover_is_transmitted_as_png_even_when_the_file_is_not() {
        // Regression: a JPEG sent with `f=100` (which means "this is a PNG") is
        // rejected by kitty, and with `q=2` we never hear about it, so the cover
        // just never appeared. Anything non-PNG is re-encoded here.
        let dir = std::env::temp_dir().join("rosadeck-images-png");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let png = dir.join("cover.png");
        image::RgbImage::from_pixel(12, 18, image::Rgb([10, 20, 30])).save(&png).unwrap();
        let bytes = read(&plan_for(&png, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)).expect("png");
        assert!(bytes.starts_with(PNG_MAGIC), "png passes through");
        assert_eq!(bytes, std::fs::read(&png).unwrap(), "untouched, not re-encoded");

        let bmp = dir.join("cover.bmp");
        image::RgbImage::from_pixel(12, 18, image::Rgb([10, 20, 30]))
            .save_with_format(&bmp, image::ImageFormat::Bmp)
            .unwrap();
        let bytes = read(&plan_for(&bmp, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)).expect("bmp is re-encoded");
        assert!(bytes.starts_with(PNG_MAGIC), "the payload kitty receives is a PNG");
        assert!(!bytes.starts_with(b"BM"), "not the raw bmp");
        // And the pixels survived the conversion.
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!((back.width(), back.height()), (12, 18));
        assert_eq!(back.get_pixel(0, 0).0, [10, 20, 30]);

        // A file that is not an image at all yields nothing (no bogus payload).
        let junk = dir.join("cover.jpg");
        std::fs::write(&junk, b"\xff\xd8\xff\xe0 truncated").unwrap();
        assert!(read(&plan_for(&junk, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP)).is_none(), "garbage is not sent");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transmitted_payload_starts_with_the_png_magic() {
        // The bytes on the wire, not just the encoder: a payload that is not a
        // PNG is a cover kitty silently drops.
        let dir = std::env::temp_dir().join("rosadeck-images-png-wire");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let bmp = dir.join("cover.bmp");
        image::RgbImage::from_pixel(12, 18, image::Rgb([1, 2, 3]))
            .save_with_format(&bmp, image::ImageFormat::Bmp)
            .unwrap();
        let plan = plan_for(&bmp, 1, 1, 4, 4, crate::cover::Treatment::Crisp, BACKDROP);
        let mut out = Vec::new();
        transmit(&mut out, &read(&plan).unwrap(), 7, &plan).unwrap();
        let text = String::from_utf8_lossy(&out).to_string();
        let payload = text.split_once(';').unwrap().1.split("\\").next().unwrap();
        let raw = decode_base64(payload);
        assert!(raw.starts_with(PNG_MAGIC), "first bytes on the wire: {raw:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Inverse of the [`base64`] encoder, for assertions only.
    fn decode_base64(s: &str) -> Vec<u8> {
        let val = |c: u8| -> u8 { match c { b'A'..=b'Z' => c - b'A', b'a'..=b'z' => c - b'a' + 26, b'0'..=b'9' => c - b'0' + 52, b'+' => 62, b'/' => 63, _ => 0 } };
        let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
        bytes.chunks(4).map(|q| {
            let b = [val(q[0]), val(*q.get(1).unwrap_or(&0)), val(*q.get(2).unwrap_or(&0)), val(*q.get(3).unwrap_or(&0))];
            let n = ((b[0] as u32) << 18) | ((b[1] as u32) << 12) | ((b[2] as u32) << 6) | b[3] as u32;
            let take = match q.len() { 4 => 3, 3 => 2, _ => 1 };
            n.to_be_bytes()[1..1 + take].to_vec()
        })
        .flatten()
        .collect()
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}