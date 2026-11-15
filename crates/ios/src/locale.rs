//! The interface language: the system's first preferred language, read once
//! at start ([`preferred_languages`], iOS only), mapped by [`language_for`].

/// The built-in language for the system's preferred languages, most
/// preferred first: Portuguese of any region gives `"pt-BR"`, anything else
/// (or no preference) `"en"`. Only the first preference counts, as iOS
/// itself would show the app in it.
pub fn language_for(preferred: &[String]) -> &'static str {
    let portuguese = preferred.first().is_some_and(|code| {
        let language = code.split(['-', '_']).next().unwrap_or_default();
        language.eq_ignore_ascii_case("pt")
    });
    if portuguese { "pt-BR" } else { "en" }
}

/// `NSLocale.preferredLanguages`: the user's languages, most preferred
/// first, as BCP 47 tags ("pt-BR", "en-US").
#[cfg(target_os = "ios")]
pub fn preferred_languages() -> Vec<String> {
    objc2_foundation::NSLocale::preferredLanguages()
        .iter()
        .map(|code| code.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|c| (*c).to_owned()).collect()
    }

    #[test]
    fn portuguese_variants_pick_pt_br() {
        assert_eq!(language_for(&langs(&["pt-PT"])), "pt-BR");
        assert_eq!(language_for(&langs(&["pt-BR"])), "pt-BR");
        assert_eq!(language_for(&langs(&["fr-FR", "pt-BR"])), "en");
        assert_eq!(language_for(&[]), "en");
        assert_eq!(language_for(&langs(&["pt"])), "pt-BR");
        // A language that merely starts with "pt" is not Portuguese.
        assert_eq!(language_for(&langs(&["ptx-XX"])), "en");
    }
}
