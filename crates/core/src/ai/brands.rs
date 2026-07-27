//! Recognising brand names in the text AtlasDrive already read from a photograph.
//!
//! This is deliberately *not* logo recognition. Apple Vision has no logo
//! classifier, and inventing one from image features would produce confident
//! nonsense — a red circle becoming "Coca-Cola". What Vision does do reliably
//! is read text, and a brand in a photograph almost always appears as text:
//! on a bottle, a shop front, a car badge, an order of service, a van.
//!
//! So a brand tag here means exactly one thing, and the interface says so:
//! **AtlasDrive read this name in the picture.** That claim is checkable, which
//! a guess from pixels would not be.
//!
//! Matching is against a fixed lexicon rather than "any capitalised word".
//! OCR of a wedding photograph is full of proper nouns — names on place cards,
//! street signs, hymn titles — and tagging all of them as brands would bury the
//! real ones. A closed list is smaller than the truth but every entry in it is
//! right.

use std::collections::BTreeSet;

/// Brands worth recognising in a British commercial and wedding archive.
///
/// Ordered by how they are written, not alphabetically, because matching is
/// done on a normalised form. Multi-word entries are matched as phrases.
const BRAND_LEXICON: &[&str] = &[
    // Drinks — the most photographed brands at any event.
    "coca-cola", "pepsi", "fanta", "sprite", "schweppes", "red bull",
    "moet", "moet & chandon", "veuve clicquot", "bollinger", "laurent-perrier",
    "dom perignon", "taittinger", "guinness", "heineken", "stella artois", "peroni", "budweiser",
    "carlsberg", "san miguel", "birra moretti", "brewdog", "camden town brewery",
    "jack daniels", "johnnie walker", "jameson", "hendricks", "gordons", "bombay sapphire", "tanqueray",
    "smirnoff", "absolut", "baileys", "malibu", "captain morgan", "aperol",
    "campari", "jagermeister", "bacardi", "glenfiddich",
    "evian", "highland spring", "buxton", "volvic", "perrier",
    // Coffee and food on the high street.
    "starbucks", "costa", "costa coffee", "caffe nero", "pret a manger",
    "greggs", "mcdonalds", "burger king", "kfc", "subway",
    "nandos", "wagamama", "pizza express", "dominos", "papa johns", "five guys", "itsu",
    // Supermarkets and retail.
    "tesco", "sainsburys", "asda", "morrisons", "aldi", "lidl",
    "waitrose", "marks & spencer", "m&s", "co-op", "iceland",
    "boots", "superdrug", "argos", "john lewis", "ikea", "b&q", "screwfix",
    "wickes", "homebase", "halfords", "currys", "primark", "h&m",
    "zara", "uniqlo", "topshop", "river island", "asos", "selfridges", "harrods",
    "amazon", "ebay", "etsy",
    // Cars and transport.
    "ford", "vauxhall", "volkswagen", "audi", "bmw", "mercedes", "mercedes-benz",
    "toyota", "honda", "nissan", "hyundai", "kia", "peugeot", "renault", "citroen", "fiat", "skoda", "volvo", "jaguar", "land rover",
    "range rover", "porsche", "ferrari", "lamborghini", "tesla", "bentley",
    "rolls-royce", "aston martin", "lotus", "mazda", "subaru", "suzuki",
    "national express", "megabus", "avanti west coast", "lner", "northern rail",
    "transpennine express", "british airways", "easyjet", "ryanair", "jet2",
    "virgin atlantic", "emirates", "dhl", "fedex", "ups", "royal mail", "hermes",
    "evri", "dpd", "yodel", "uber", "addison lee",
    // Technology and cameras — a photographer's own kit shows up constantly.
    "apple", "iphone", "ipad", "macbook", "samsung", "google", "microsoft",
    "sony", "canon", "nikon", "fujifilm", "olympus", "panasonic", "leica",
    "pentax", "sigma", "tamron", "godox", "profoto", "elinchrom", "manfrotto",
    "gitzo", "peak design", "gopro", "dji", "lexar", "sandisk", "westcott",
    "bose", "sonos", "jbl", "dell", "hp", "lenovo", "asus", "acer",
    "nintendo", "playstation", "xbox", "lego",
    // Sport and clothing.
    "nike", "adidas", "puma", "reebok", "new balance", "under armour", "asics",
    "converse", "dr martens", "clarks", "timberland", "the north face",
    "patagonia", "berghaus", "regatta", "barbour", "burberry", "gucci", "prada",
    "louis vuitton", "chanel", "hugo boss", "ted baker", "ralph lauren",
    "tommy hilfiger", "levis", "superdry", "jack wills",
    "manchester united", "manchester city", "liverpool fc", "arsenal", "chelsea",
    "tottenham", "everton", "rangers", "celtic",
    // Banks, telecoms, media and utilities — signage and sponsorship.
    "barclays", "hsbc", "natwest", "lloyds", "santander", "halifax", "nationwide",
    "monzo", "starling", "revolut", "visa", "mastercard", "amex",
    "american express", "paypal", "vodafone", "o2", "ee", "bt", "sky",
    "virgin media", "talktalk", "bbc", "itv", "channel 4", "netflix", "disney",
    "spotify", "british gas", "eon", "octopus energy", "shell", "bp",
    "esso", "texaco", "national trust", "english heritage", "nhs",
];

/// A brand found in a photograph's text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BrandHit {
    /// The brand as it is written in the lexicon, not as OCR happened to read
    /// it — so "MOËT", "Moet" and "moët" all become one tag.
    pub name: String,
}

/// Brands that are also ordinary English words, place names or surnames.
///
/// These are only accepted when the photograph shows them in **capitals**,
/// which is how a shop front, a van livery or a bottle label writes them, and
/// is not how prose writes them. Without this rule a real archive produced
/// "Next Collection Time" as the retailer Next, and "THE THREE FISHES" — a pub
/// — as the mobile network Three. Both were tagged before this existed.
///
/// Brands whose everyday meaning swamps the brand entirely (next, three, seat,
/// gap, mini, bolt, corona, iceland, vans, beats) are not in the lexicon at
/// all: no capitalisation rule can rescue them.
const AMBIGUOUS: &[&str] = &[
    "apple", "boots", "shell", "visa", "costa", "lotus", "ford", "sky", "subway",
    "converse", "chelsea", "arsenal", "tottenham", "everton", "rangers", "celtic",
    "hermes", "bt", "ee", "hp", "ups", "o2", "amex", "sigma",
    "gucci", "mercedes", "jaguar",
];

fn is_ambiguous(brand: &str) -> bool {
    AMBIGUOUS.contains(&brand)
}

/// Shortest name worth matching.
const MIN_BRAND_LEN: usize = 2;

/// Find brand names in text read from a photograph.
///
/// Matching is on whole words, case- and accent-insensitively, so "MOET &
/// CHANDON" on a bottle and "Moët" on a menu produce the same tag. Longer
/// names win: "Land Rover" must not also report "Rover". Names that are also
/// ordinary words must additionally appear in capitals — see [`AMBIGUOUS`].
pub fn detect(text: &str) -> Vec<BrandHit> {
    let (haystack, was_upper) = normalise_with_case(text);
    if haystack.trim().is_empty() {
        return Vec::new();
    }

    // Longest first, so a phrase claims its words before its parts can.
    let mut lexicon: Vec<&str> = BRAND_LEXICON.to_vec();
    lexicon.sort_by_key(|b| std::cmp::Reverse(b.len()));

    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut claimed: Vec<(usize, usize)> = Vec::new();

    for brand in lexicon {
        if brand.len() < MIN_BRAND_LEN {
            continue;
        }
        let needle = normalise(brand);
        for (start, end) in whole_word_matches(&haystack, &needle) {
            if claimed.iter().any(|(s, e)| start < *e && *s < end) {
                continue; // already part of a longer brand
            }
            if is_ambiguous(brand) {
                // Every letter of the match must have been a capital.
                let shouted = was_upper[start..end]
                    .iter()
                    .zip(haystack[start..end].bytes())
                    .all(|(up, b)| !b.is_ascii_alphabetic() || *up);
                if !shouted {
                    continue;
                }
            }
            claimed.push((start, end));
            found.insert(canonical(brand));
        }
    }

    found.into_iter().map(|name| BrandHit { name }).collect()
}

/// Normalise, and report which bytes came from an upper-case letter.
///
/// The mask is byte-aligned with the returned string so a match's span can be
/// checked for capitalisation without re-scanning the original.
fn normalise_with_case(s: &str) -> (String, Vec<bool>) {
    let mut out = String::with_capacity(s.len());
    let mut upper: Vec<bool> = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let was_upper = ch.is_uppercase();
        let folded = fold(ch);
        if folded.is_alphanumeric() {
            let before = out.len();
            out.extend(folded.to_lowercase());
            upper.resize(before, false);
            upper.resize(out.len(), was_upper);
        } else if matches!(folded, '\'' | '\u{2019}') {
            // dropped, not spaced
        } else {
            out.push(' ');
            upper.push(false);
        }
    }
    // Collapse runs of spaces, keeping the mask aligned.
    let mut squeezed = String::with_capacity(out.len());
    let mut mask = Vec::with_capacity(upper.len());
    let mut last_space = true; // trims the leading space too
    for (i, b) in out.bytes().enumerate() {
        let is_space = b == b' ';
        if is_space && last_space {
            continue;
        }
        squeezed.push(b as char);
        mask.push(upper.get(i).copied().unwrap_or(false));
        last_space = is_space;
    }
    while squeezed.ends_with(' ') {
        squeezed.pop();
        mask.pop();
    }
    (squeezed, mask)
}

pub(crate) fn fold(ch: char) -> char {
    match ch {
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'à' | 'á' | 'â' | 'ä' | 'À' | 'Á' | 'Â' | 'Ä' => 'a',
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
        'ò' | 'ó' | 'ô' | 'ö' | 'Ò' | 'Ó' | 'Ô' | 'Ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'š' | 'Š' => 's',
        c => c,
    }
}

/// Lower-case, fold accents, and reduce punctuation to spaces.
fn normalise(s: &str) -> String {
    normalise_with_case(s).0
}

/// Byte ranges where `needle` appears in `haystack` on word boundaries.
fn whole_word_matches(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        let before_ok = start == 0 || bytes[start - 1] == b' ';
        let after_ok = end == bytes.len() || bytes[end] == b' ';
        if before_ok && after_ok {
            out.push((start, end));
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    out
}

/// How a brand is written as a tag: lower case, spaces kept, "&" spelled out.
///
/// Tags elsewhere in the catalogue are lower-case single tokens or
/// hyphen-joined, and a brand should look like one of them rather than like a
/// sentence.
fn canonical(brand: &str) -> String {
    let cleaned = brand
        .replace('&', "and")
        .replace(['\u{2019}', '\''], "")
        .to_lowercase();
    cleaned.split_whitespace().collect::<Vec<_>>().join("-")
}

/// How many brands the lexicon knows, for the diagnostics report.
pub fn lexicon_size() -> usize {
    BRAND_LEXICON.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<String> {
        detect(text).into_iter().map(|b| b.name).collect()
    }

    #[test]
    fn reads_a_brand_off_a_bottle() {
        assert_eq!(names("COCA-COLA 330ml"), vec!["coca-cola"]);
        assert_eq!(names("Champagne Moët & Chandon"), vec!["moet-and-chandon"]);
    }

    /// The same brand written three ways must produce one tag, or the tag
    /// cloud fills with near-duplicates that each match a third of the photos.
    #[test]
    fn spelling_and_accents_collapse_to_one_tag() {
        assert_eq!(names("MOET"), names("Moët"));
        assert_eq!(names("Sainsbury's"), names("SAINSBURYS"));
        assert_eq!(names("McDonald's"), names("mcdonalds"));
    }

    /// A longer brand must claim its words: "Land Rover" is not also "Rover",
    /// and a photograph of a Range Rover is not a photograph of a Mini.
    #[test]
    fn the_longest_name_wins() {
        assert_eq!(names("LAND ROVER DEFENDER"), vec!["land-rover"]);
        let hits = names("Range Rover parked outside");
        assert_eq!(hits, vec!["range-rover"]);
    }

    /// The failure that would make this feature worthless: matching inside
    /// ordinary words. "Nextdoor" is not Next; "Bpm" is not BP.
    #[test]
    fn does_not_match_inside_other_words() {
        assert!(names("nextdoor neighbours").is_empty());
        assert!(names("120 bpm").is_empty());
        assert!(names("subwaystation").is_empty());
        assert!(names("threefold increase").is_empty());
    }

    #[test]
    fn finds_several_brands_in_one_photograph() {
        let hits = names("Order of service — bar sponsored by Guinness and Peroni, cars by Audi");
        assert!(hits.contains(&"guinness".to_string()));
        assert!(hits.contains(&"peroni".to_string()));
        assert!(hits.contains(&"audi".to_string()));
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn empty_and_meaningless_text_finds_nothing() {
        assert!(names("").is_empty());
        assert!(names("   \n\t ").is_empty());
        assert!(names("aaaa bbbb cccc").is_empty());
    }

    /// Real OCR is messy: line breaks, stray punctuation, mixed case.
    #[test]
    fn survives_the_shape_of_real_ocr_output() {
        let ocr = "WELCOME TO\nTHE  TESCO  EXTRA\n\nOpen 24 hrs.\nBOOTS pharmacy →";
        let hits = names(ocr);
        assert!(hits.contains(&"tesco".to_string()));
        assert!(hits.contains(&"boots".to_string()));
    }

    /// A brand tag makes a claim about the photograph, so a hit has to be a
    /// hit — every entry in the lexicon must be findable by its own name.
    #[test]
    fn every_brand_in_the_lexicon_can_be_found() {
        for brand in BRAND_LEXICON {
            // A name that is also an ordinary word is only accepted in
            // capitals, which is how signage writes it.
            let written = if is_ambiguous(brand) { brand.to_uppercase() } else { brand.to_string() };
            let hits = names(&format!("sign reading {written} here"));
            assert!(
                !hits.is_empty(),
                "{brand} is in the lexicon but cannot be detected"
            );
        }
    }

    /// Everything in AMBIGUOUS must actually be in the lexicon, or the rule is
    /// guarding something that can never match and reads as protection it is
    /// not providing.
    #[test]
    fn every_ambiguous_name_is_a_real_lexicon_entry() {
        for name in AMBIGUOUS {
            assert!(
                BRAND_LEXICON.contains(name),
                "{name} is marked ambiguous but is not a brand in the lexicon"
            );
        }
    }

    /// The exact false positives a real archive produced before the
    /// capitalisation rule existed. Both were tagged; neither is a brand.
    #[test]
    fn ordinary_prose_is_not_mistaken_for_a_brand() {
        // "Next Collection Time" on a wedding post box.
        assert!(names("Wedding Post Box Next Collection Time 7.30pm").is_empty());
        // "THE THREE FISHES" — a pub, not the mobile network.
        assert!(names("THE THREE FISHES").is_empty());
        // Ordinary uses of words that are also brands.
        assert!(names("she pulled on her boots and walked").is_empty());
        assert!(names("an apple from the garden").is_empty());
        assert!(names("a shell on the beach").is_empty());
        assert!(names("the ford across the river").is_empty());
    }

    /// And the rule must not throw away the real thing: signage and labels
    /// write these names in capitals.
    #[test]
    fn the_same_names_are_kept_when_the_photograph_shouts_them() {
        assert_eq!(names("BOOTS PHARMACY"), vec!["boots"]);
        assert_eq!(names("SHELL"), vec!["shell"]);
        assert_eq!(names("APPLE STORE"), vec!["apple"]);
        assert_eq!(names("FORD TRANSIT"), vec!["ford"]);
    }

    /// Tag names are what the owner clicks in the tag cloud, so they must be
    /// shaped like the other tags there: lower case, no spaces, no punctuation
    /// that would look like a typo.
    #[test]
    fn tag_names_are_shaped_like_every_other_tag() {
        for brand in BRAND_LEXICON {
            let tag = canonical(brand);
            assert!(!tag.contains(' '), "{tag} has a space");
            assert!(!tag.contains('\''), "{tag} has an apostrophe");
            assert!(!tag.contains('&'), "{tag} has an ampersand");
            assert_eq!(tag, tag.to_lowercase(), "{tag} is not lower case");
            assert!(!tag.is_empty());
        }
    }

    #[test]
    fn the_lexicon_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for brand in BRAND_LEXICON {
            assert!(seen.insert(normalise(brand)), "duplicate entry: {brand}");
        }
    }
}
