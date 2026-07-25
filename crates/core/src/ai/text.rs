//! Local text encoder for natural-language visual search.
//!
//! `docs/07_VISUAL_SEARCH_AND_TAGGING.md` requires that a natural-language query
//! be "embedded locally using the same compatible visual-text model family as
//! the image embeddings". This module is the text half of the
//! [`super::local::LocalHeuristicEngine`] family: it renders a query into the
//! same coarse 4×4 colour-layout grid that the image encoder reduces every
//! photograph to, and that grid is then embedded by the *identical* function.
//! Query vectors therefore live in exactly the same space as image vectors —
//! never a second, silently incompatible one.
//!
//! Scope, stated plainly: this is a deterministic lexicon over visual priors,
//! not a learned CLIP text tower. It resolves colour, lighting, setting and
//! "is there a person in the middle" — the signals the heuristic image encoder
//! actually records — and reports honest coverage so the caller can tell how
//! much of the query it understood. A learned local text encoder replaces it by
//! registering an engine with the same [`super::Capability::TextEmbedding`],
//! under its own `model_id`/`model_version`, without any database change.

use image::{Rgb, RgbImage};

/// Side length of the conceptual grid. Must match the image encoder's grid.
pub(crate) const GRID: u32 = 4;

/// The visual prior a single vocabulary term contributes.
#[derive(Debug, Clone, Copy)]
struct VisualPrior {
    /// Colour of the upper half of the frame.
    top: [f32; 3],
    /// Colour of the lower half of the frame.
    bottom: [f32; 3],
    /// Optional central-subject colour (e.g. a skin tone for "portrait").
    subject: Option<[f32; 3]>,
    /// Term implies a monochrome photograph.
    grayscale: bool,
}

const fn prior(top: [f32; 3], bottom: [f32; 3]) -> VisualPrior {
    VisualPrior { top, bottom, subject: None, grayscale: false }
}

const fn with_subject(top: [f32; 3], bottom: [f32; 3], subject: [f32; 3]) -> VisualPrior {
    VisualPrior { top, bottom, subject: Some(subject), grayscale: false }
}

const fn mono(top: [f32; 3], bottom: [f32; 3]) -> VisualPrior {
    VisualPrior { top, bottom, subject: None, grayscale: true }
}

/// A skin tone consistent with the detector's `is_skin` heuristic, so a query
/// mentioning people leans towards frames that actually contain skin-tone
/// regions.
const SKIN: [f32; 3] = [205.0, 160.0, 130.0];

/// Terms that carry no visual meaning and must not count against coverage.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "in", "on", "at", "to", "with", "and", "or", "for", "from", "my",
    "our", "their", "his", "her", "some", "any", "all", "is", "are", "was", "were", "be",
    "photo", "photos", "photograph", "photographs", "picture", "pictures", "image", "images",
    "pic", "pics", "shot", "shots", "showing", "show", "me", "find", "search", "standing",
    "sitting", "front", "near", "next", "that", "this", "it", "there",
];

/// The lexicon. Each entry maps synonyms to one visual prior.
///
/// Ordering is irrelevant — every matched prior is composited with equal weight,
/// so "red car" blends the colour term and the object term rather than letting
/// one silently win.
#[allow(clippy::type_complexity)]
const VOCAB: &[(&[&str], VisualPrior)] = &[
    // ---- Settings -------------------------------------------------------
    (
        &["outdoor", "outdoors", "outside", "garden", "park", "field", "meadow", "countryside"],
        prior([120.0, 170.0, 220.0], [90.0, 140.0, 70.0]),
    ),
    (&["sky", "cloud", "clouds", "cloudy"], prior([135.0, 180.0, 230.0], [150.0, 190.0, 225.0])),
    (
        &["sea", "ocean", "beach", "coast", "seaside", "shore", "sand"],
        prior([120.0, 175.0, 225.0], [225.0, 205.0, 160.0]),
    ),
    (
        &["water", "lake", "river", "pool", "pond", "canal"],
        prior([110.0, 150.0, 190.0], [70.0, 110.0, 160.0]),
    ),
    (&["snow", "snowy", "winter", "ski", "ice"], prior([200.0, 215.0, 235.0], [240.0, 242.0, 248.0])),
    (
        &["sunset", "sunrise", "dusk", "dawn", "golden"],
        prior([240.0, 150.0, 70.0], [120.0, 80.0, 60.0]),
    ),
    (&["night", "evening", "nighttime", "stars", "starry"], prior([25.0, 30.0, 55.0], [20.0, 22.0, 35.0])),
    (
        &["forest", "tree", "trees", "woods", "woodland", "grass", "jungle", "hedge"],
        prior([80.0, 120.0, 70.0], [60.0, 95.0, 50.0]),
    ),
    (
        &["mountain", "mountains", "hill", "hills", "valley", "cliff"],
        prior([150.0, 175.0, 200.0], [110.0, 105.0, 90.0]),
    ),
    (&["desert", "dune", "dunes"], prior([200.0, 190.0, 160.0], [215.0, 190.0, 140.0])),
    (&["flower", "flowers", "blossom", "roses"], prior([150.0, 190.0, 120.0], [200.0, 120.0, 150.0])),
    (
        &["indoor", "indoors", "room", "house", "home", "kitchen", "lounge", "living", "bedroom"],
        prior([150.0, 130.0, 110.0], [130.0, 110.0, 95.0]),
    ),
    (
        &["church", "chapel", "cathedral", "building", "buildings", "city", "street", "town", "urban", "architecture"],
        prior([170.0, 180.0, 190.0], [140.0, 140.0, 140.0]),
    ),
    (
        &["school", "classroom", "graduation", "college", "university"],
        with_subject([160.0, 155.0, 150.0], [135.0, 130.0, 125.0], SKIN),
    ),

    // ---- Objects --------------------------------------------------------
    (
        &["car", "cars", "vehicle", "truck", "bus", "van"],
        with_subject([160.0, 175.0, 190.0], [120.0, 120.0, 125.0], [140.0, 140.0, 145.0]),
    ),
    (
        &["bike", "bikes", "bicycle", "bicycles", "cycling", "motorbike", "motorcycle"],
        with_subject([150.0, 180.0, 200.0], [110.0, 130.0, 100.0], [90.0, 95.0, 105.0]),
    ),
    (
        &["boat", "boats", "ship", "sailing", "harbour", "harbor", "yacht"],
        with_subject([140.0, 175.0, 215.0], [70.0, 110.0, 155.0], [200.0, 200.0, 200.0]),
    ),
    (
        &["dog", "dogs", "cat", "cats", "pet", "pets", "animal", "animals", "horse", "horses"],
        with_subject([140.0, 160.0, 120.0], [110.0, 130.0, 90.0], [150.0, 120.0, 90.0]),
    ),
    (
        &["food", "meal", "dinner", "lunch", "table", "cooking"],
        with_subject([150.0, 130.0, 110.0], [170.0, 140.0, 110.0], [190.0, 150.0, 100.0]),
    ),
    (
        &["document", "documents", "paper", "letter", "certificate", "newspaper", "page"],
        prior([235.0, 233.0, 228.0], [232.0, 230.0, 225.0]),
    ),

    // ---- People ---------------------------------------------------------
    (
        &["portrait", "portraits", "face", "faces", "headshot"],
        with_subject([120.0, 110.0, 105.0], [110.0, 100.0, 95.0], SKIN),
    ),
    (
        &["people", "person", "family", "group", "crowd", "friends", "everyone", "couple"],
        with_subject([140.0, 140.0, 135.0], [120.0, 115.0, 110.0], SKIN),
    ),
    (
        &["child", "children", "kid", "kids", "baby", "babies", "toddler", "boy", "girl", "son", "daughter"],
        with_subject([150.0, 145.0, 135.0], [130.0, 125.0, 115.0], [215.0, 170.0, 140.0]),
    ),
    (
        &["wedding", "bride", "groom", "marriage"],
        with_subject([200.0, 195.0, 190.0], [185.0, 180.0, 175.0], [215.0, 175.0, 145.0]),
    ),
    (
        &["birthday", "party", "present", "presents", "gift", "gifts", "christmas", "cake", "celebration"],
        with_subject([180.0, 120.0, 110.0], [150.0, 100.0, 90.0], [220.0, 180.0, 120.0]),
    ),
    (
        &["sport", "sports", "football", "cricket", "game", "match", "team", "race"],
        prior([150.0, 185.0, 215.0], [80.0, 140.0, 70.0]),
    ),

    // ---- Era and finish -------------------------------------------------
    (
        &["blackandwhite", "monochrome", "bw", "greyscale", "grayscale"],
        mono([150.0, 150.0, 150.0], [120.0, 120.0, 120.0]),
    ),
    (&["old", "vintage", "antique", "historic", "retro"], mono([160.0, 160.0, 160.0], [130.0, 130.0, 130.0])),
    (&["sepia", "faded"], prior([190.0, 165.0, 125.0], [160.0, 135.0, 100.0])),

    // ---- Light and season ------------------------------------------------
    (&["bright", "sunny", "daylight", "summer", "sunshine"], prior([215.0, 225.0, 240.0], [190.0, 200.0, 180.0])),
    (&["dark", "dim", "shadow", "shadows"], prior([50.0, 50.0, 55.0], [40.0, 40.0, 45.0])),
    (&["autumn", "fall"], prior([200.0, 150.0, 80.0], [150.0, 110.0, 60.0])),
    (&["spring"], prior([170.0, 200.0, 230.0], [120.0, 170.0, 90.0])),

    // ---- Bare colour terms ----------------------------------------------
    (&["red"], prior([200.0, 60.0, 50.0], [200.0, 60.0, 50.0])),
    (&["blue"], prior([60.0, 90.0, 200.0], [60.0, 90.0, 200.0])),
    (&["green"], prior([60.0, 150.0, 70.0], [60.0, 150.0, 70.0])),
    (&["yellow"], prior([225.0, 205.0, 70.0], [225.0, 205.0, 70.0])),
    (&["orange"], prior([230.0, 140.0, 50.0], [230.0, 140.0, 50.0])),
    (&["purple", "violet"], prior([130.0, 70.0, 170.0], [130.0, 70.0, 170.0])),
    (&["pink"], prior([230.0, 150.0, 180.0], [230.0, 150.0, 180.0])),
    (&["brown"], prior([140.0, 100.0, 60.0], [140.0, 100.0, 60.0])),
    (&["white"], prior([240.0, 240.0, 240.0], [240.0, 240.0, 240.0])),
    (&["black"], prior([25.0, 25.0, 25.0], [25.0, 25.0, 25.0])),
    (&["grey", "gray"], prior([128.0, 128.0, 128.0], [128.0, 128.0, 128.0])),
];

/// Multi-word phrases folded into a single token before lookup, so "black and
/// white" is not shredded into an unrelated colour pair by the tokenizer.
const PHRASES: &[(&str, &str)] = &[
    ("black and white", "blackandwhite"),
    ("black & white", "blackandwhite"),
    ("black-and-white", "blackandwhite"),
    ("blackandwhite", "blackandwhite"),
];

/// How much of a query the lexicon understood, and the grid it produced.
#[derive(Debug, Clone)]
pub struct RenderedQuery {
    /// The 4×4 conceptual frame, ready for the shared image embedder.
    pub grid: RgbImage,
    /// Fraction of meaningful query terms that matched the lexicon, in [0,1].
    /// Zero means the query carried no visual signal and callers should fall
    /// back to text/metadata search rather than present a meaningless ranking.
    pub coverage: f32,
    /// The lexicon terms that matched, for explaining a result to the user.
    pub matched_terms: Vec<String>,
}

/// Render a natural-language query into the shared 4×4 conceptual frame.
pub fn render_query(query: &str) -> RenderedQuery {
    let normalized = normalize(query);
    let tokens: Vec<String> = normalized
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect();

    let mut meaningful = 0usize;
    let mut matched_terms: Vec<String> = Vec::new();
    let mut priors: Vec<VisualPrior> = Vec::new();

    for token in &tokens {
        if STOPWORDS.contains(&token.as_str()) {
            continue;
        }
        meaningful += 1;
        if let Some((canonical, p)) = lookup(token) {
            if !matched_terms.iter().any(|t| t == canonical) {
                matched_terms.push(canonical.to_string());
            }
            priors.push(p);
        }
    }

    let coverage = if meaningful == 0 {
        0.0
    } else {
        (priors.len() as f32 / meaningful as f32).clamp(0.0, 1.0)
    };

    RenderedQuery { grid: composite(&priors), coverage, matched_terms }
}

/// Lower-case, fold known phrases, and strip punctuation to spaces.
fn normalize(query: &str) -> String {
    let mut s = query.to_lowercase();
    for (phrase, token) in PHRASES {
        if s.contains(phrase) {
            s = s.replace(phrase, token);
        }
    }
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect()
}

/// Look a token up, retrying once without a plural `s`.
///
/// Returns the entry's *first* synonym as the canonical label, so "mountains",
/// "mountain" and "hills" all explain themselves to the user as one term.
fn lookup(token: &str) -> Option<(&'static str, VisualPrior)> {
    if let Some(hit) = lookup_exact(token) {
        return Some(hit);
    }
    match token.strip_suffix('s') {
        Some(singular) if singular.len() >= 3 => lookup_exact(singular),
        _ => None,
    }
}

fn lookup_exact(token: &str) -> Option<(&'static str, VisualPrior)> {
    for (terms, p) in VOCAB {
        if terms.contains(&token) {
            return Some((terms[0], *p));
        }
    }
    None
}

/// Composite the matched priors into one 4×4 frame.
///
/// A neutral mid-grey carries a small baseline weight so an unmatched or
/// partially matched query degrades towards "no opinion" rather than towards
/// whichever single term happened to match.
///
/// That baseline is deliberately light. The embedding preserves absolute
/// lightness, so a heavy grey baseline would drag every query towards mid-grey
/// and make "night" and "snow" both rank a mid-tone frame highest — the exact
/// failure the brightness anchor exists to prevent.
fn composite(priors: &[VisualPrior]) -> RgbImage {
    const BASE: [f32; 3] = [128.0, 128.0, 128.0];
    const BASE_WEIGHT: f32 = 0.2;
    const HALF_WEIGHT: f32 = 1.0;
    const SUBJECT_WEIGHT: f32 = 1.5;

    let mut sums = [[[0f32; 3]; GRID as usize]; GRID as usize];
    let mut weights = [[0f32; GRID as usize]; GRID as usize];

    let mut add = |row: usize, col: usize, colour: [f32; 3], w: f32| {
        for c in 0..3 {
            sums[row][col][c] += colour[c] * w;
        }
        weights[row][col] += w;
    };

    for row in 0..GRID as usize {
        for col in 0..GRID as usize {
            add(row, col, BASE, BASE_WEIGHT);
        }
    }

    let mut grayscale = false;
    for p in priors {
        grayscale |= p.grayscale;
        for row in 0..GRID as usize {
            let colour = if row < (GRID as usize) / 2 { p.top } else { p.bottom };
            for col in 0..GRID as usize {
                add(row, col, colour, HALF_WEIGHT);
            }
        }
        if let Some(subject) = p.subject {
            // Central 2×2 block: where a photographed subject usually sits.
            for row in 1..3 {
                for col in 1..3 {
                    add(row, col, subject, SUBJECT_WEIGHT);
                }
            }
        }
    }

    let mut img = RgbImage::new(GRID, GRID);
    for row in 0..GRID as usize {
        for col in 0..GRID as usize {
            let w = weights[row][col].max(f32::EPSILON);
            let mut rgb = [sums[row][col][0] / w, sums[row][col][1] / w, sums[row][col][2] / w];
            if grayscale {
                let lum = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
                rgb = [lum, lum, lum];
            }
            img.put_pixel(
                col as u32,
                row as u32,
                Rgb([clamp_u8(rgb[0]), clamp_u8(rgb[1]), clamp_u8(rgb[2])]),
            );
        }
    }
    img
}

fn clamp_u8(v: f32) -> u8 {
    v.clamp(0.0, 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avg(img: &RgbImage) -> [f32; 3] {
        let mut s = [0f32; 3];
        for p in img.pixels() {
            for c in 0..3 {
                s[c] += p[c] as f32;
            }
        }
        let n = (img.width() * img.height()) as f32;
        [s[0] / n, s[1] / n, s[2] / n]
    }

    #[test]
    fn grid_is_the_shared_four_by_four_frame() {
        let r = render_query("beach");
        assert_eq!(r.grid.dimensions(), (GRID, GRID));
    }

    #[test]
    fn unknown_query_reports_zero_coverage() {
        let r = render_query("qwertyuiop zxcvbnm");
        assert_eq!(r.coverage, 0.0);
        assert!(r.matched_terms.is_empty());
    }

    #[test]
    fn stopwords_do_not_penalise_coverage() {
        // "images" is a stopword, so a fully understood query stays at 1.0.
        let r = render_query("bike images");
        assert_eq!(r.coverage, 1.0);
        assert_eq!(r.matched_terms, vec!["bike"]);
    }

    #[test]
    fn partial_coverage_is_reported_honestly() {
        let r = render_query("bike wobblegong");
        assert!(r.coverage > 0.0 && r.coverage < 1.0, "coverage was {}", r.coverage);
    }

    #[test]
    fn black_and_white_phrase_survives_tokenizing() {
        let r = render_query("old black and white family portrait");
        assert!(r.matched_terms.iter().any(|t| t == "blackandwhite"));
        // Monochrome request must produce a grey frame, not a colour one.
        let a = avg(&r.grid);
        assert!((a[0] - a[1]).abs() < 1.5 && (a[1] - a[2]).abs() < 1.5, "not grey: {a:?}");
    }

    #[test]
    fn colour_terms_shift_the_frame_the_right_way() {
        let red = avg(&render_query("red").grid);
        let blue = avg(&render_query("blue").grid);
        assert!(red[0] > red[2], "red query should be red-dominant: {red:?}");
        assert!(blue[2] > blue[0], "blue query should be blue-dominant: {blue:?}");
    }

    #[test]
    fn plural_terms_resolve_to_the_singular_entry() {
        let a = render_query("mountains");
        assert_eq!(a.matched_terms, vec!["mountain"]);
    }

    #[test]
    fn people_queries_place_a_skin_tone_subject_centrally() {
        let r = render_query("family portrait");
        let centre = r.grid.get_pixel(1, 1);
        let corner = r.grid.get_pixel(0, 0);
        // The centre should be warmer (more red-vs-blue) than the corner.
        let centre_warmth = centre[0] as i32 - centre[2] as i32;
        let corner_warmth = corner[0] as i32 - corner[2] as i32;
        assert!(
            centre_warmth > corner_warmth,
            "centre {centre_warmth} should be warmer than corner {corner_warmth}"
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        let a = render_query("children opening presents");
        let b = render_query("children opening presents");
        assert_eq!(a.grid.as_raw(), b.grid.as_raw());
        assert_eq!(a.coverage, b.coverage);
    }
}
