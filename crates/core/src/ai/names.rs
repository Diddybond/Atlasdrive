//! Turning names printed on things in a photograph into tags.
//!
//! A photographer's archive is full of names that are not people: a bottle
//! behind a bar, a van in a car park, a shop front, a magazine on a table, the
//! pub the reception was in. Vision already reads that text. This module
//! decides which of it is a *name* and turns each one into its own tag.
//!
//! What a name tag means, exactly: **AtlasDrive read this on something in the
//! picture.** It is never inferred from image content. Apple Vision has no logo
//! classifier, and guessing a brand from pixels would produce a confident claim
//! nobody could check — a red circle becoming Coca-Cola. Text can be checked by
//! looking at the photograph.
//!
//! The hard part is not finding capitalised words; it is refusing the ones that
//! are not names. Real OCR from this archive produced "Next Collection Time"
//! from a wedding post box and "WEDDING POST BOX" from the same sign. Neither is
//! a name. The rules below are each there because of a specific thing the
//! catalogue actually did.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::ai::brands;

/// Every ordinary English word, so a name can be recognised by *not* being one.
///
/// This replaced a hand-written list of a few hundred words, which was the
/// wrong shape for the job. Asked to tag every name it could read, AtlasDrive
/// tagged 643 of 741 photographs, offering "real", "peace", "squirt" and
/// "laugh" as names — all of them words printed in capitals on packaging that
/// the short list happened not to contain. A hand-written list can only ever be
/// as good as the words someone remembered to add.
///
/// The list is committed to the repository rather than read from the host, so a
/// build on any machine produces the same tags. It is the system dictionary
/// filtered to plain ASCII words of 2 to 16 letters.
const ENGLISH_WORDS: &str = include_str!("english_words.txt");

fn english() -> &'static Vec<&'static str> {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| ENGLISH_WORDS.lines().collect())
}

/// True when this is an ordinary English word rather than a name.
///
/// Note what this deliberately does *not* do: it does not reject a name simply
/// because it is unfamiliar. Unfamiliar is exactly what a name looks like.
///
/// Inflected and British forms are checked too. The dictionary holds "award"
/// but not "awarded", and it is American, so it has "moisturize" and not
/// "moisturise" — without this, ordinary words printed on packaging came back
/// as names.
fn is_english_word(word: &str) -> bool {
    let plain = plain_lower(word);
    if plain.len() < 2 {
        return false;
    }
    if in_dictionary(&plain) {
        return true;
    }
    for form in word_forms(&plain) {
        if in_dictionary(&form) {
            return true;
        }
    }
    false
}

fn in_dictionary(word: &str) -> bool {
    english().binary_search(&word).is_ok()
}

/// Plausible dictionary forms of an inflected or British-spelled word.
fn word_forms(w: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stem = |suffix: &str, replacement: &str| {
        if w.ends_with(suffix) && w.len() - suffix.len() >= 3 {
            out.push(format!("{}{replacement}", &w[..w.len() - suffix.len()]));
        }
    };
    // Plurals and past/continuous forms. "es" is deliberately not stripped as
    // a unit: it turned "Bedes" — St Bede's — into "bed".
    stem("ies", "y");
    stem("s", "");
    stem("ing", "");
    stem("ing", "e");
    stem("ed", "");
    stem("ed", "e");
    stem("ers", "");
    stem("er", "");
    stem("ly", "");

    // British spellings the American dictionary lacks.
    for (british, american) in
        [("isation", "ization"), ("ising", "izing"), ("ised", "ized"), ("ise", "ize"), ("our", "or")]
    {
        if w.contains(british) {
            out.push(w.replace(british, american));
        }
    }
    out
}

/// How often each two-letter sequence occurs in English, per million.
///
/// Derived from the same dictionary, so nothing extra is shipped to support it.
fn bigram_rates() -> &'static [u32; 676] {
    static RATES: OnceLock<[u32; 676]> = OnceLock::new();
    RATES.get_or_init(|| {
        let mut counts = [0u64; 676];
        let mut total = 0u64;
        for word in english() {
            let b = word.as_bytes();
            for pair in b.windows(2) {
                if pair[0].is_ascii_lowercase() && pair[1].is_ascii_lowercase() {
                    let i = (pair[0] - b'a') as usize * 26 + (pair[1] - b'a') as usize;
                    counts[i] += 1;
                    total += 1;
                }
            }
        }
        let mut rates = [0u32; 676];
        for (i, c) in counts.iter().enumerate() {
            rates[i] = ((c * 1_000_000) / total.max(1)) as u32;
        }
        rates
    })
}

/// Rarest two-letter sequence in a word, per million, or `None` if too short.
fn rarest_bigram(word: &str) -> Option<u32> {
    let b: Vec<u8> = word.bytes().filter(|c| c.is_ascii_lowercase()).collect();
    if b.len() < 2 {
        return None;
    }
    let rates = bigram_rates();
    b.windows(2)
        .map(|p| rates[(p[0] - b'a') as usize * 26 + (p[1] - b'a') as usize])
        .min()
}

/// Below this, a two-letter sequence essentially does not occur in English.
///
/// Measured, not guessed. Against this archive's own stored text, OCR garble
/// — "vbebimrtodady", "pecwdegdeaxl", "snidhrlouzxby" — scored between 0 and 6
/// per million, while every real name scored 39 or above: Atlas Copco was the
/// lowest at 39, then handcrafted at 50, Ribcaged at 106, Slingsby at 133.
const MIN_BIGRAM_RATE: u32 = 20;

/// True when a word AtlasDrive does not recognise is more likely to be OCR
/// misreading a picture than a name printed on something.
///
/// Only ever applied to words that are not in the dictionary: a real English
/// word is a real word whatever its letters look like.
fn looks_like_misread_text(word: &str) -> bool {
    let plain = plain_lower(word);
    // Anything outside the Latin alphabet came from Vision misreading shapes.
    if plain.chars().any(|c| !c.is_ascii_lowercase()) {
        return true;
    }
    match rarest_bigram(&plain) {
        Some(rate) => rate < MIN_BIGRAM_RATE,
        None => true,
    }
}

/// Words that are never a name on their own, and that a name is not made of.
///
/// Deliberately ordinary vocabulary rather than a stopword list: the test a
/// candidate has to pass is "is any part of this an unusual word", and common
/// nouns are what makes "WEDDING POST BOX" fail it while "TANQUERAY" passes.
const COMMON_WORDS: &[&str] = &[
    // Function words — also stripped from the ends of a phrase.
    "the", "a", "an", "and", "of", "or", "by", "at", "in", "on", "to", "for",
    "with", "from", "our", "your", "my", "his", "her", "its", "their", "this",
    "that", "these", "those", "is", "are", "was", "were", "be", "been", "am",
    "it", "we", "you", "they", "he", "she", "not", "no", "yes", "all", "any",
    "more", "most", "some", "such", "than", "then", "there", "here", "when",
    "where", "who", "what", "why", "how", "if", "as", "so", "but", "up", "out",
    "off", "over", "under", "into", "onto", "per", "via", "new", "old",
    // Everyday nouns and verbs that turn up on signs and in captions.
    "wedding", "weddings", "post", "box", "boxes", "time", "times", "date",
    "dates", "day", "days", "night", "week", "month", "year", "years", "hour",
    "hours", "min", "mins", "minute", "minutes", "collection", "open", "closed",
    "welcome", "please", "thank", "thanks", "you", "sale", "sales", "price",
    "prices", "free", "now", "today", "tomorrow", "call", "phone", "email",
    "web", "www", "com", "uk", "co", "org", "net", "info", "home", "page",
    "menu", "food", "drink", "drinks", "bar", "cafe", "shop", "store", "stores",
    "market", "street", "road", "lane", "avenue", "close", "way", "park",
    "house", "hall", "church", "school", "hotel", "inn", "room", "rooms",
    "toilet", "toilets", "exit", "entrance", "entry", "car", "cars", "parking",
    "bus", "train", "station", "stop", "left", "right", "north", "south",
    "east", "west", "centre", "center", "city", "town", "village", "county",
    "england", "scotland", "wales", "ireland", "britain", "british",
    "photo", "photos", "photograph", "photographs", "picture", "pictures",
    "image", "images", "copy", "copies", "print", "prints", "page", "pages",
    "name", "names", "number", "numbers", "size", "sizes", "colour", "color",
    "black", "white", "red", "blue", "green", "yellow", "gold", "silver",
    "large", "small", "big", "little", "long", "short", "high", "low", "best",
    "great", "good", "bad", "first", "last", "next", "final", "only", "just",
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    "ten", "hundred", "thousand", "million",
    "monday", "tuesday", "wednesday", "thursday", "friday", "saturday",
    "sunday", "january", "february", "march", "april", "may", "june", "july",
    "august", "september", "october", "november", "december",
    "mr", "mrs", "ms", "miss", "dr", "sir", "lady", "lord",
    "bride", "groom", "guest", "guests", "family", "friends", "love", "happy",
    "birthday", "party", "celebration", "reception", "ceremony", "service",
    "order", "orders", "table", "tables", "seat", "seats", "chair", "chairs",
    "cake", "flowers", "flower", "gift", "gifts", "card", "cards", "sign",
    "signs", "book", "books", "list", "lists", "note", "notes", "message",
    "messages", "wish", "wishes", "special", "sharing", "share", "make",
    "made", "take", "taken", "get", "got", "see", "seen", "look", "looking",
    "come", "coming", "going", "goes", "here", "there",
];

/// Words stripped from the *ends* of a candidate, because they belong to the
/// sentence rather than to the name.
///
/// Only function words, never ordinary nouns. Stripping every common word from
/// the edges reduced "THE THREE FISHES" to "fishes", because "three" is
/// everyday vocabulary — it is still part of the pub's name.
const EDGE_WORDS: &[&str] = &[
    "the", "a", "an", "and", "of", "or", "by", "at", "in", "on", "to", "for",
    "with", "from", "our", "your", "my", "is", "are", "was", "were", "be",
    "it", "we", "you", "they", "as", "so", "but", "please", "thank", "thanks",
    "welcome", "here", "there", "now",
];

fn is_edge_word(word: &str) -> bool {
    EDGE_WORDS.contains(&plain_lower(word).as_str())
}

/// Longest run of words treated as one name.
///
/// Four covers "THE OLD RED LION" and "MOET AND CHANDON"; beyond that a run of
/// capitals is a sentence in a headline, not a name.
const MAX_NAME_WORDS: usize = 4;

/// Shortest unrecognised word that can stand as a name on its own.
///
/// Three-letter fragments off a label — "cin", "daf", "ome" — are OCR clipping
/// a longer word far more often than they are a name. Known brands are matched
/// through the lexicon first, so NHS, DHL and BMW are unaffected by this.
const MIN_SOLO_LEN: usize = 4;

/// A name read off something in a photograph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NameHit {
    /// The tag, lower case and hyphen-joined — `jack-daniels`, `berghaus`.
    pub tag: String,
    /// True when this matched AtlasDrive's list of known brands, so the
    /// spelling has been canonicalised and variants merge.
    pub known_brand: bool,
}

/// Find the names printed on things in a photograph, from its text.
///
/// A candidate is a run of capitalised words. It becomes a tag when at least
/// one of those words is *not* ordinary vocabulary — which is what separates
/// "TANQUERAY" and "THE THREE FISHES" from "WEDDING POST BOX" and "Next
/// Collection Time", all four of which this archive produced.
pub fn detect(text: &str) -> Vec<NameHit> {
    let mut found: BTreeSet<(String, bool)> = BTreeSet::new();

    // Known brands are matched against the raw text first, under their own
    // rules. They have to be: "NHS" and "DHL" carry no vowel and "M&S" is not
    // even letters, so the general rules below — which exist to refuse OCR
    // noise — would throw them out.
    for brand in brands::detect(text) {
        found.insert((brand.name, true));
    }

    for line in text.lines() {
        for run in capitalised_runs(line) {
            if let Some(hit) = name_from_run(&run) {
                found.insert((hit.tag, hit.known_brand));
            }
        }
    }

    // A known brand and a raw reading that canonicalise to the same tag are one
    // tag; prefer the brand flag so the interface can say the spelling is
    // trusted.
    let mut by_tag: std::collections::BTreeMap<String, bool> = Default::default();
    for (tag, known) in found {
        let entry = by_tag.entry(tag).or_insert(known);
        *entry = *entry || known;
    }
    by_tag.into_iter().map(|(tag, known_brand)| NameHit { tag, known_brand }).collect()
}

/// Split a line into runs of consecutive capitalised words.
///
/// A word breaks the run if it is lower case, contains a digit, or is not
/// letters — so "Dorco Classic by Dorco, from £8.74" yields "Dorco Classic"
/// and "Dorco", not one long smear across the price.
fn capitalised_runs(line: &str) -> Vec<Vec<String>> {
    let mut runs: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for raw in line.split_whitespace() {
        let word = trim_word(raw);
        if word.is_empty() {
            // Punctuation alone ends the run: "Aimee & Kent" is two names on a
            // card, not one four-word phrase.
            push_run(&mut runs, &mut current);
            continue;
        }
        if is_capitalised_word(&word) {
            current.push(word);
            if current.len() == MAX_NAME_WORDS {
                push_run(&mut runs, &mut current);
            }
        } else {
            push_run(&mut runs, &mut current);
        }
    }
    push_run(&mut runs, &mut current);
    runs
}

fn push_run(runs: &mut Vec<Vec<String>>, current: &mut Vec<String>) {
    if !current.is_empty() {
        runs.push(std::mem::take(current));
    }
}

/// Strip surrounding punctuation, keeping internal apostrophes and hyphens.
fn trim_word(raw: &str) -> String {
    raw.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

/// True when a word could be part of a name.
///
/// Requires an initial capital, letters only, and a vowel. The vowel test is
/// what keeps OCR garble — "гісс", "Bt", "GG" — out of the tag cloud; a real
/// word in a Latin script almost always has one.
fn is_capitalised_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_uppercase() {
        return false;
    }
    if word.chars().any(|c| c.is_numeric()) {
        return false;
    }
    if !word.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '\u{2019}' || c == '-') {
        return false;
    }
    if !word.chars().any(|c| "aeiouyAEIOUY".contains(c) || !c.is_ascii()) {
        return false;
    }
    word.chars().filter(|c| c.is_alphabetic()).count() >= 2
}

/// Decide whether a run of capitalised words is a name, and what to call it.
fn name_from_run(run: &[String]) -> Option<NameHit> {
    // Function words at either end belong to the sentence, not the name:
    // "THE THREE FISHES" is the Three Fishes.
    let mut words: Vec<&String> = run.iter().collect();
    while words.first().is_some_and(|w| is_edge_word(w)) && words.len() > 1 {
        words.remove(0);
    }
    while words.last().is_some_and(|w| is_edge_word(w)) && words.len() > 1 {
        words.pop();
    }
    if words.is_empty() {
        return None;
    }

    // The rule that does the work: something in here has to be a word English
    // does not have. A run made entirely of dictionary words is a phrase off a
    // sign — "WEDDING POST BOX", "OPEN 24 HOURS" — not a name.
    //
    // Known brands are checked first below, so the ones that *are* ordinary
    // words (Apple, Shell, Boots, Next) are still recognised; this rule only
    // governs names AtlasDrive has never heard of.
    // Words Vision misread are dropped before the run is judged, so one
    // smear of noise cannot carry a whole phrase into the catalogue.
    let words: Vec<&String> = words
        .into_iter()
        .filter(|w| is_english_word(w) || !looks_like_misread_text(w))
        .collect();
    if words.is_empty() {
        return None;
    }

    if words.iter().all(|w| is_english_word(w) || is_common(w)) {
        // ...unless a known brand is hiding in it, which the lexicon settles.
        let joined = words.iter().map(|w| w.as_str()).collect::<Vec<_>>().join(" ");
        if let Some(brand) = brands::detect(&joined).into_iter().next() {
            return Some(NameHit { tag: brand.name, known_brand: true });
        }
        return None;
    }

    let joined = words.iter().map(|w| w.as_str()).collect::<Vec<_>>().join(" ");

    // A known brand canonicalises, so "MOËT", "Moet" and "moet & chandon" all
    // land on one tag instead of three that each match a third of the archive.
    if let Some(brand) = brands::detect(&joined).into_iter().next() {
        return Some(NameHit { tag: brand.name, known_brand: true });
    }

    // A single word standing alone has to be long enough to be worth a tag.
    if words.len() == 1 && words[0].chars().filter(|c| c.is_alphabetic()).count() < MIN_SOLO_LEN {
        return None;
    }

    Some(NameHit { tag: to_tag(&joined), known_brand: false })
}

fn is_common(word: &str) -> bool {
    COMMON_WORDS.contains(&plain_lower(word).as_str())
}

/// Letters only, lower case, with common Latin accents folded.
///
/// Folding matters in both directions: "Moët" has to reach the dictionary and
/// the brand lexicon as "moet", while a word carrying letters from outside the
/// Latin alphabet is Vision misreading shapes and should stay unfoldable so it
/// can be recognised as such.
fn plain_lower(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphabetic())
        .map(brands::fold)
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Shape a name like every other tag in the catalogue: lower case, joined by
/// hyphens, no punctuation that would look like a typo.
fn to_tag(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '\'' | '\u{2019}' => '\0',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .filter(|c| *c != '\0')
        .collect();
    cleaned
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(text: &str) -> Vec<String> {
        detect(text).into_iter().map(|h| h.tag).collect()
    }

    /// The names the owner actually wanted as tags, read off bottles behind a
    /// bar and signage in the background.
    #[test]
    fn reads_names_off_things_in_the_picture() {
        assert!(tags("TANQUERAY LONDON DRY GIN").contains(&"tanqueray".to_string()));
        assert!(tags("BERGHAUS").contains(&"berghaus".to_string()));
        assert!(tags("ASDA").contains(&"asda".to_string()));
        assert!(tags("BACARDI").contains(&"bacardi".to_string()));
    }

    /// Names AtlasDrive has never heard of must work exactly as well as ones
    /// it knows — that is the whole point of dropping the fixed list.
    #[test]
    fn a_name_it_has_never_seen_becomes_a_tag() {
        assert_eq!(tags("DORCO"), vec!["dorco"]);
        assert!(tags("Langshawhead Farm").contains(&"langshawhead-farm".to_string()));
        assert!(tags("KRUGHAMMER STOUT").contains(&"krughammer-stout".to_string()));
    }

    /// The two false positives this archive really produced. Every word in
    /// each is ordinary vocabulary, so neither is a name.
    #[test]
    fn a_phrase_made_only_of_ordinary_words_is_not_a_name() {
        assert!(tags("Next Collection Time").is_empty());
        assert!(tags("WEDDING POST BOX").is_empty());
        assert!(tags("Thank you for sharing our special day").is_empty());
        assert!(tags("OPEN 24 HOURS").is_empty());
    }

    /// But an ordinary word sitting next to an unusual one is part of a name.
    #[test]
    fn an_ordinary_word_beside_an_unusual_one_is_kept() {
        assert!(tags("THE THREE FISHES").contains(&"three-fishes".to_string()));
        assert!(tags("Langshawhead Farm").contains(&"langshawhead-farm".to_string()));
    }

    /// OCR garble must not reach the tag cloud. These are verbatim from the
    /// archive's own stored text.
    #[test]
    fn ocr_noise_is_refused() {
        assert!(tags("гісс").is_empty());
        assert!(tags("GG").is_empty(), "no vowel");
        assert!(tags("Bt L").is_empty(), "too short and no vowel");
        assert!(tags("E8.74").is_empty(), "contains digits");
        assert!(tags("XZ").is_empty());
    }

    /// A known brand keeps its canonical spelling however OCR read it, so the
    /// variants do not each become a tag matching a fraction of the archive.
    #[test]
    fn known_brands_still_merge_their_spellings() {
        let a = tags("MOET");
        let b = tags("Moët");
        assert_eq!(a, b);
        assert_eq!(a, vec!["moet"]);
        assert!(detect("MOET")[0].known_brand);
        assert!(!detect("DORCO")[0].known_brand);
    }

    /// Punctuation separates one name from the next, so two names on a shelf
    /// do not fuse into a phrase that matches neither.
    #[test]
    fn punctuation_separates_one_name_from_the_next() {
        let t = tags("TANQUERAY & SLINGSBY");
        assert!(t.contains(&"tanqueray".to_string()), "{t:?}");
        assert!(t.contains(&"slingsby".to_string()), "{t:?}");
        assert!(!t.iter().any(|x| x.contains("tanqueray-slingsby")), "{t:?}");
    }

    /// People's first names are ordinary vocabulary and stay out of the tag
    /// cloud. They are not lost — who is in a photograph is what the People
    /// screen and face recognition are for, and a place card reading "Aimee &
    /// Kent" would otherwise put a tag on every table shot at that wedding.
    #[test]
    fn first_names_on_place_cards_do_not_become_subject_tags() {
        assert!(tags("Aimee & Kent").is_empty());
        assert!(tags("Sarah and Michael").is_empty());
    }

    /// A run longer than four words is a headline, not a name.
    #[test]
    fn a_long_shout_is_not_treated_as_one_name() {
        for tag in tags("EXCLUSIVE INTERVIEW WITH THE PRINCE IN HIS PALACE GARDEN") {
            assert!(
                tag.split('-').count() <= MAX_NAME_WORDS,
                "{tag} is too long to be a name"
            );
        }
    }

    /// Tags must look like the others in the cloud, because they sit beside
    /// them and get clicked the same way.
    #[test]
    fn tags_are_shaped_like_every_other_tag() {
        for tag in tags("GENTLEMEN'S QUARTERLY and JACK DANIEL'S") {
            assert_eq!(tag, tag.to_lowercase());
            assert!(!tag.contains(' '), "{tag} has a space");
            assert!(!tag.contains('\''), "{tag} has an apostrophe");
            assert!(!tag.is_empty());
        }
    }

    /// Inflected and British forms of ordinary words are still ordinary. Each
    /// of these came back as a "name" from the real catalogue before the
    /// dictionary learned to stem.
    #[test]
    fn inflected_and_british_spellings_are_still_ordinary_words() {
        assert!(tags("AWARDED").is_empty());
        assert!(tags("AWARDS").is_empty());
        assert!(tags("LIGHTS").is_empty());
        assert!(tags("WORDS").is_empty());
        assert!(tags("CONTAINS").is_empty());
        assert!(tags("EXFOLIATING").is_empty());
    }

    /// But stemming must not eat a name: "Bedes" — St Bede's — is not "bed".
    #[test]
    fn stemming_does_not_swallow_a_name() {
        assert!(tags("ST BEDES").contains(&"bedes".to_string()));
    }

    /// Vision misreading a picture as letters must not reach the tag cloud.
    /// These are verbatim from the archive's own stored text.
    #[test]
    fn text_vision_misread_is_refused() {
        for junk in ["VBEBIMRTODADY", "PECWDEGDEAXL", "SNIDHRLOUZXBY", "ZPLACGIHE"] {
            assert!(tags(junk).is_empty(), "{junk} should not be a tag");
        }
    }

    /// And a real name must survive the same test, however unfamiliar.
    #[test]
    fn unfamiliar_but_real_names_survive_the_noise_filter() {
        for name in ["RIBCAGED", "ASTAXANTHIN", "SLINGSBY", "TANQUERAY", "BERGHAUS", "CHAMBORD"] {
            assert!(!tags(name).is_empty(), "{name} was wrongly refused");
        }
    }

    /// Three-letter fragments are OCR clipping a longer word far more often
    /// than they are a name — but a known short brand still comes through.
    #[test]
    fn short_fragments_are_refused_but_known_short_brands_are_not() {
        assert!(tags("CIN").is_empty());
        assert!(tags("DAF").is_empty());
        assert!(tags("OME").is_empty());
        assert_eq!(tags("NHS"), vec!["nhs"], "a known short brand still counts");
        assert_eq!(tags("DHL"), vec!["dhl"]);
    }

    #[test]
    fn empty_text_finds_nothing() {
        assert!(tags("").is_empty());
        assert!(tags("   \n\t  ").is_empty());
        assert!(tags("all lower case words only").is_empty());
    }

    /// Real multi-line OCR, as stored in the catalogue.
    #[test]
    fn works_on_the_shape_of_real_ocr_output() {
        let ocr = "WELCOME TO\nTHE TESCO EXTRA\n\nOpen 24 hrs.\nBOOTS pharmacy\nBERGHAUS";
        let t = tags(ocr);
        assert!(t.contains(&"tesco".to_string()), "{t:?}");
        assert!(t.contains(&"boots".to_string()), "{t:?}");
        assert!(t.contains(&"berghaus".to_string()), "{t:?}");
        // "WELCOME TO" is ordinary vocabulary throughout.
        assert!(!t.iter().any(|x| x.contains("welcome")), "{t:?}");
    }

    /// The same photograph read twice gives the same tags, and each tag once.
    #[test]
    fn the_result_is_stable_and_free_of_duplicates() {
        let text = "BACARDI BACARDI bacardi Bacardi";
        let t = tags(text);
        assert_eq!(t.len(), t.iter().collect::<BTreeSet<_>>().len());
        assert_eq!(t, tags(text));
    }
}
