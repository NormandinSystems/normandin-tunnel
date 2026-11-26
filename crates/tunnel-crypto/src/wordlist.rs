//! The fixed word list connection codes are drawn from.
//!
//! This is EFF Short Wordlist #2: 1,296 words where every word has a unique
//! three-letter prefix, so autocomplete can finish a word after three letters.
//! The same English list is used in every language so that codes always match
//! across devices.

pub use crate::wordlist_data::WORDS;

/// Number of words in the list (6^4).
pub const WORD_COUNT: usize = 1296;

/// Letters needed before a word is uniquely identified.
pub const UNIQUE_PREFIX_LEN: usize = 3;

/// Reduces typed text to the form used for matching: ASCII letters, lower case.
///
/// Hyphens are dropped so that the one hyphenated word in the list (`yo-yo`)
/// matches whether it is typed as `yo-yo`, `yoyo` or `yo yo`.
pub fn normalize(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Returns the word at `index`, if it exists.
pub fn word(index: u16) -> Option<&'static str> {
    WORDS.get(usize::from(index)).copied()
}

/// Finds the word a typed token stands for.
///
/// A token matches when it is the whole word, or a prefix of at least
/// [`UNIQUE_PREFIX_LEN`] letters. Because three-letter prefixes are unique, a
/// prefix can never match two words.
pub fn lookup(token: &str) -> Option<u16> {
    let token = normalize(token);
    if token.is_empty() {
        return None;
    }
    WORDS.iter().enumerate().find_map(|(index, word)| {
        let word = normalize(word);
        let matches =
            word == token || (token.len() >= UNIQUE_PREFIX_LEN && word.starts_with(&token));
        matches.then_some(index as u16)
    })
}

/// Lists up to `limit` words starting with `prefix`, for autocomplete.
pub fn complete(prefix: &str, limit: usize) -> Vec<&'static str> {
    let prefix = normalize(prefix);
    if prefix.is_empty() {
        return Vec::new();
    }
    WORDS
        .iter()
        .filter(|word| normalize(word).starts_with(&prefix))
        .take(limit)
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn list_has_1296_unique_words() {
        assert_eq!(WORDS.len(), WORD_COUNT);
        let unique: HashSet<_> = WORDS.iter().collect();
        assert_eq!(unique.len(), WORD_COUNT);
    }

    #[test]
    fn every_word_has_a_unique_three_letter_prefix() {
        let prefixes: HashSet<String> = WORDS
            .iter()
            .map(|w| normalize(w).chars().take(UNIQUE_PREFIX_LEN).collect())
            .collect();
        assert_eq!(prefixes.len(), WORD_COUNT);
    }

    #[test]
    fn words_are_lowercase_ascii_and_sorted() {
        for word in WORDS {
            assert!(word.len() >= UNIQUE_PREFIX_LEN, "{word} is too short");
            assert!(
                word.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{word} has unexpected characters"
            );
        }
        let mut sorted = WORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            WORDS.to_vec(),
            "the list order is part of the protocol"
        );
    }

    #[test]
    fn lookup_accepts_whole_words_prefixes_and_any_case() {
        let index = lookup("aardvark").expect("first word");
        assert_eq!(index, 0);
        assert_eq!(lookup("AARDVARK"), Some(0));
        assert_eq!(lookup("aar"), Some(0));
        assert_eq!(lookup("aa"), None, "two letters are not enough");
        assert_eq!(lookup("aardvarks"), None, "longer than the word");
        assert_eq!(lookup(""), None);
        assert_eq!(lookup("12"), None);
    }

    #[test]
    fn hyphenated_word_matches_in_every_spelling() {
        let index = WORDS
            .iter()
            .position(|w| *w == "yo-yo")
            .expect("yo-yo is in the list");
        assert_eq!(lookup("yo-yo"), Some(index as u16));
        assert_eq!(lookup("yoyo"), Some(index as u16));
        assert_eq!(lookup("YoY"), Some(index as u16));
    }

    #[test]
    fn complete_offers_matches() {
        assert_eq!(complete("aar", 5), vec!["aardvark"]);
        assert!(complete("a", 500).len() > 10);
        assert!(complete("", 5).is_empty());
        assert!(complete("zzzz", 5).is_empty());
    }
}
