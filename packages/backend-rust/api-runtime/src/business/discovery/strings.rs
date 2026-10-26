use unicode_normalization::UnicodeNormalization;

pub(super) fn like_escape(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '\\' | '%' | '_') {
            result.push('\\');
        }
        result.push(character);
    }
    result
}

/// ECMAScript normalize/lowercase retain lone UTF-16 surrogates. Normalize
/// scalar runs independently, preserving each intervening surrogate unchanged.
fn map_scalar_runs(units: &[u16], transform: impl Fn(&str) -> String) -> Vec<u16> {
    let mut result = Vec::new();
    let mut run = String::new();
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(character) => run.push(character),
            Err(error) => {
                result.extend(transform(&run).encode_utf16());
                run.clear();
                result.push(error.unpaired_surrogate());
            }
        }
    }
    result.extend(transform(&run).encode_utf16());
    result
}

pub(super) fn lowercase(units: &[u16]) -> Vec<u16> {
    map_scalar_runs(units, str::to_lowercase)
}

pub(super) fn normalize_for_search(units: &[u16]) -> Vec<u16> {
    map_scalar_runs(units, |run| run.nfkc().collect::<String>().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn sql_like_wildcards_remain_literal() {
        assert_eq!(like_escape("a%_\\b"), "a\\%\\_\\\\b");
    }

    #[test]
    fn node_unicode_search_examples_preserve_surrogates_and_context() {
        assert_eq!(lowercase(&units("ΟΣ ΟΣΑ İ")), units("ος οσα i\u{307}"));
        assert_eq!(
            normalize_for_search(&units("Ｈｅｌｌｏ K ① ㍿")),
            units("hello k 1 株式会社")
        );
        assert_eq!(
            lowercase(&[0xd800, 0x41, 0xdc00]),
            vec![0xd800, 0x61, 0xdc00]
        );
        assert_eq!(
            normalize_for_search(&[0xd800, 0xff21, 0xdc00]),
            vec![0xd800, 0x61, 0xdc00]
        );
        // Unicode 18 assigns compatibility mappings to these formerly
        // unassigned characters; the current Bun runtime uses Unicode 17.
        assert_eq!(
            normalize_for_search(&units("\u{558}\u{209d}")),
            units("\u{558}\u{209d}")
        );
    }
}
