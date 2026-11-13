//! Text recognition with the ocrs engine and its two neural-network models
//! (text detection and text recognition), which are compiled into the exe.
//!
//! The models are by Robert Knight (https://github.com/robertknight/ocrs-models)
//! and licensed CC-BY-SA 4.0; they are used unmodified.

use std::sync::OnceLock;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;
use rten_imageproc::{Rect, RotatedRect};
use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, Tensor};

use super::gray::Gray;

static DETECTION_MODEL: &[u8] = include_bytes!("../../assets/ocr/text-detection.onnx");
static RECOGNITION_MODEL: &[u8] = include_bytes!("../../assets/ocr/text-recognition.onnx");

/// Loading the models takes a moment; do it once.
fn engine() -> Result<&'static OcrEngine, String> {
    static ENGINE: OnceLock<Result<OcrEngine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let detection = Model::load_static_slice(DETECTION_MODEL).map_err(|e| e.to_string())?;
            let recognition = Model::load_static_slice(RECOGNITION_MODEL).map_err(|e| e.to_string())?;
            OcrEngine::new(OcrEngineParams {
                detection_model: Some(detection),
                recognition_model: Some(recognition),
                ..Default::default()
            })
            .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| format!("The text recognition engine could not start: {e}"))
}

/// An axis-aligned box in page pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
    pub fn center(&self) -> (f32, f32) {
        ((self.left + self.right) / 2.0, (self.top + self.bottom) / 2.0)
    }
    pub fn union(&self, o: &Bounds) -> Bounds {
        Bounds {
            left: self.left.min(o.left),
            top: self.top.min(o.top),
            right: self.right.max(o.right),
            bottom: self.bottom.max(o.bottom),
        }
    }
}

/// Find words on a deskewed page whose text is about `text_h` pixels tall.
///
/// The detection model looks at 600 x 800 pixels at a time, so the page is
/// scaled to put the text at a size the model reads well and then examined
/// tile by tile (with overlap, keeping each tile's middle), rather than
/// squeezing the whole page into one model input as ocrs does by default.
pub fn detect_words(page: &Gray, text_h: f32) -> Result<Vec<Bounds>, String> {
    let engine = engine()?;
    let scale = (DETECT_TEXT_H / text_h).clamp(0.15, 1.0);
    let small = page.scale(scale);
    let (w, h) = (small.width, small.height);
    let (tw, th) = (TILE_W, TILE_H);
    let mut prob = vec![0f32; w * h];
    let starts = |len: usize, tile: usize| -> Vec<usize> {
        if len <= tile {
            return vec![0];
        }
        let step = tile - 2 * OVERLAP;
        let mut v: Vec<usize> = (0..).map(|i| i * step).take_while(|&s| s + tile < len).collect();
        v.push(len - tile);
        v
    };
    for &ty in &starts(h, th) {
        for &tx in &starts(w, tw) {
            let tile = small.crop(tx as i64, ty as i64, tw, th);
            let source = ImageSource::from_bytes(&tile.px, (tw as u32, th as u32)).map_err(|e| e.to_string())?;
            let input = engine.prepare_input(source).map_err(|e| e.to_string())?;
            let map = engine.detect_text_pixels(&input).map_err(|e| e.to_string())?;
            // Keep the middle of each tile; edges come from its neighbours.
            let x_from = if tx == 0 { 0 } else { OVERLAP };
            let y_from = if ty == 0 { 0 } else { OVERLAP };
            for y in y_from..th.min(h - ty) {
                for x in x_from..tw.min(w - tx) {
                    prob[(ty + y) * w + tx + x] = map[[y, x]];
                }
            }
        }
    }
    let threshold = engine.detection_threshold();
    let mask: Vec<u8> = prob.iter().map(|&p| u8::from(p > threshold)).collect();
    let blobs = super::prep::blobs(&super::prep::Binary { width: w, height: h, ink: mask });
    let inv = 1.0 / scale;
    Ok(blobs
        .into_iter()
        .filter(|b| b.pixels >= 6)
        .map(|b| {
            // The model marks a slightly shrunken word area: grow it back.
            let grow = (b.height() as f32 * 0.25).max(1.5);
            Bounds {
                left: ((b.left as f32 - grow) * inv).max(0.0),
                top: ((b.top as f32 - grow) * inv).max(0.0),
                right: (((b.right + 1) as f32 + grow) * inv).min(page.width as f32),
                bottom: (((b.bottom + 1) as f32 + grow) * inv).min(page.height as f32),
            }
        })
        .collect())
}

/// Text height (pixels) the detection model sees best.
const DETECT_TEXT_H: f32 = 12.0;
const TILE_W: usize = 600;
const TILE_H: usize = 800;
const OVERLAP: usize = 48;

/// The recognition model on its own, for scoring how readable lines are.
fn scorer() -> Result<&'static Model, String> {
    static MODEL: OnceLock<Result<Model, String>> = OnceLock::new();
    MODEL
        .get_or_init(|| Model::load_static_slice(RECOGNITION_MODEL).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| format!("The text recognition engine could not start: {e}"))
}

/// How confidently the recognition model reads each image (one line of
/// text each): the average over its output steps of the best character's
/// log-probability. Higher is more readable.
fn readability(lines: &[Gray]) -> Result<Vec<f32>, String> {
    const HEIGHT: usize = 64;
    let model = scorer()?;
    let widths: Vec<usize> = lines.iter().map(|l| (l.width * HEIGHT / l.height.max(1)).clamp(16, 1200)).collect();
    let max_w = widths.iter().copied().max().unwrap_or(16).next_multiple_of(50);
    let mut input = NdTensor::full([lines.len(), 1, HEIGHT, max_w], -0.5f32);
    for (i, (line, &w)) in lines.iter().zip(&widths).enumerate() {
        let scaled = line.resize(w, HEIGHT);
        for y in 0..HEIGHT {
            for x in 0..w {
                input[[i, 0, y, x]] = f32::from(scaled.px[y * w + x]) / 255.0 - 0.5;
            }
        }
    }
    let output: Tensor<f32> = model
        .run_one(input.view().into(), None)
        .map_err(|e| e.to_string())?
        .try_into()
        .map_err(|e: rten::TryFromValueError| e.to_string())?;
    let shape = output.shape();
    if shape.len() != 3 || shape[1] != lines.len() {
        return Err("unexpected recognition output".into());
    }
    let (steps, classes) = (shape[0], shape[2]);
    let data = output.to_vec();
    Ok(widths
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            // Over the steps where it sees a character (not the blank,
            // class 0): how sure it is of that character.
            let used = (steps * w / max_w).clamp(1, steps);
            let (mut total, mut count) = (0f32, 0usize);
            for t in 0..used {
                let base = (t * lines.len() + i) * classes;
                let probs = &data[base..base + classes];
                let (best, p) =
                    probs.iter().enumerate().fold((0, f32::MIN), |a, (k, &v)| if v > a.1 { (k, v) } else { a });
                if best != 0 {
                    total += p;
                    count += 1;
                }
            }
            if count == 0 { -5.0 } else { total / count as f32 }
        })
        .collect())
}

/// Whether the page's text is upside down, judged by how much better the
/// recognition model reads a sample of its lines one way up than the other.
pub fn is_upside_down(page: &Gray, words: &[Bounds], th: f32) -> Result<bool, String> {
    let mut lines: Vec<Bounds> = super::table::text_lines(words)
        .iter()
        .map(|l| l.iter().skip(1).fold(l[0], |a, w| a.union(w)))
        .filter(|b| b.width() >= th * 3.0 && b.height() >= th * 0.6 && b.height() <= th * 2.5)
        .collect();
    if lines.len() < 2 {
        return Ok(false);
    }
    lines.sort_by(|a, b| b.width().total_cmp(&a.width()));
    lines.truncate(10);
    let pad = th * 0.15;
    // The start of each line is enough (and quicker than all of it).
    let crops: Vec<Gray> = lines
        .iter()
        .map(|b| {
            let h = b.height() + 2.0 * pad;
            page.crop(
                (b.left - pad) as i64,
                (b.top - pad) as i64,
                (b.width() + 2.0 * pad).min(h * 12.0) as usize,
                h as usize,
            )
        })
        .collect();
    let n = lines.len() as f32;
    let upright: f32 = readability(&crops)?.iter().sum::<f32>() / n;
    // Clearly readable as it is: no need to try the other way up.
    if upright > -0.15 {
        return Ok(false);
    }
    let flipped: Vec<Gray> = crops.iter().map(|c| c.rotate90(2)).collect();
    let turned: f32 = readability(&flipped)?.iter().sum::<f32>() / n;
    Ok(turned > upright + 0.1)
}

/// A page prepared for recognition.
pub struct Prepared {
    input: ocrs::OcrInput,
}

/// Prepare a page for reading lines from it.
pub fn prepare(page: &Gray) -> Result<Prepared, String> {
    let engine = engine()?;
    let source =
        ImageSource::from_bytes(&page.px, (page.width as u32, page.height as u32)).map_err(|e| e.to_string())?;
    Ok(Prepared { input: engine.prepare_input(source).map_err(|e| e.to_string())? })
}

/// Read lines of text, each given as its bounding box on the page. Returns
/// each line's characters with where each starts (x on the page).
pub fn read_lines(page: &Prepared, lines: &[Bounds]) -> Result<Vec<Vec<(char, f32)>>, String> {
    let engine = engine()?;
    let rects: Vec<Vec<RotatedRect>> =
        lines.iter().map(|b| vec![RotatedRect::from_rect(Rect::from_tlbr(b.top, b.left, b.bottom, b.right))]).collect();
    let read = engine.recognize_text(&page.input, &rects).map_err(|e| e.to_string())?;
    Ok(read
        .into_iter()
        .map(|line| {
            line.map(|l| l.chars().iter().map(|c| (c.char, c.rect.left() as f32)).collect()).unwrap_or_default()
        })
        .collect())
}
