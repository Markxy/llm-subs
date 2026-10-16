//! Language table used for the target language picker, sidecar naming and
//! container metadata.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    /// ISO 639-1 two-letter code, used in file names (`Movie.de.srt`).
    pub code: &'static str,
    /// ISO 639-2/B three-letter code, used for stream metadata in containers.
    pub code3: &'static str,
    pub english_name: &'static str,
    pub native_name: &'static str,
}

impl Language {
    pub fn display_name(&self) -> String {
        if self.english_name == self.native_name {
            self.english_name.to_string()
        } else {
            format!("{} ({})", self.english_name, self.native_name)
        }
    }
}

macro_rules! languages {
    ($( $code:literal, $code3:literal, $english:literal, $native:literal; )*) => {
        &[ $( Language { code: $code, code3: $code3, english_name: $english, native_name: $native }, )* ]
    };
}

static LANGUAGES: &[Language] = languages! {
    "af", "afr", "Afrikaans", "Afrikaans";
    "sq", "alb", "Albanian", "Shqip";
    "am", "amh", "Amharic", "አማርኛ";
    "ar", "ara", "Arabic", "العربية";
    "hy", "arm", "Armenian", "Հայերեն";
    "az", "aze", "Azerbaijani", "Azərbaycan dili";
    "eu", "baq", "Basque", "Euskara";
    "be", "bel", "Belarusian", "Беларуская";
    "bn", "ben", "Bengali", "বাংলা";
    "bs", "bos", "Bosnian", "Bosanski";
    "bg", "bul", "Bulgarian", "Български";
    "my", "bur", "Burmese", "မြန်မာဘာသာ";
    "ca", "cat", "Catalan", "Català";
    "zh", "chi", "Chinese (Simplified)", "简体中文";
    "zh-TW", "chi", "Chinese (Traditional)", "繁體中文";
    "hr", "hrv", "Croatian", "Hrvatski";
    "cs", "cze", "Czech", "Čeština";
    "da", "dan", "Danish", "Dansk";
    "nl", "dut", "Dutch", "Nederlands";
    "en", "eng", "English", "English";
    "et", "est", "Estonian", "Eesti";
    "fil", "fil", "Filipino", "Filipino";
    "fi", "fin", "Finnish", "Suomi";
    "fr", "fre", "French", "Français";
    "gl", "glg", "Galician", "Galego";
    "ka", "geo", "Georgian", "ქართული";
    "de", "ger", "German", "Deutsch";
    "el", "gre", "Greek", "Ελληνικά";
    "gu", "guj", "Gujarati", "ગુજરાતી";
    "he", "heb", "Hebrew", "עברית";
    "hi", "hin", "Hindi", "हिन्दी";
    "hu", "hun", "Hungarian", "Magyar";
    "is", "ice", "Icelandic", "Íslenska";
    "id", "ind", "Indonesian", "Bahasa Indonesia";
    "ga", "gle", "Irish", "Gaeilge";
    "it", "ita", "Italian", "Italiano";
    "ja", "jpn", "Japanese", "日本語";
    "kn", "kan", "Kannada", "ಕನ್ನಡ";
    "kk", "kaz", "Kazakh", "Қазақ тілі";
    "km", "khm", "Khmer", "ភាសាខ្មែរ";
    "ko", "kor", "Korean", "한국어";
    "ku", "kur", "Kurdish", "Kurdî";
    "lo", "lao", "Lao", "ລາວ";
    "lv", "lav", "Latvian", "Latviešu";
    "lt", "lit", "Lithuanian", "Lietuvių";
    "lb", "ltz", "Luxembourgish", "Lëtzebuergesch";
    "mk", "mac", "Macedonian", "Македонски";
    "ms", "may", "Malay", "Bahasa Melayu";
    "ml", "mal", "Malayalam", "മലയാളം";
    "mt", "mlt", "Maltese", "Malti";
    "mr", "mar", "Marathi", "मराठी";
    "mn", "mon", "Mongolian", "Монгол";
    "ne", "nep", "Nepali", "नेपाली";
    "no", "nor", "Norwegian", "Norsk";
    "fa", "per", "Persian", "فارسی";
    "pl", "pol", "Polish", "Polski";
    "pt", "por", "Portuguese", "Português";
    "pt-BR", "por", "Portuguese (Brazil)", "Português do Brasil";
    "pa", "pan", "Punjabi", "ਪੰਜਾਬੀ";
    "ro", "rum", "Romanian", "Română";
    "ru", "rus", "Russian", "Русский";
    "sr", "srp", "Serbian", "Српски";
    "si", "sin", "Sinhala", "සිංහල";
    "sk", "slo", "Slovak", "Slovenčina";
    "sl", "slv", "Slovenian", "Slovenščina";
    "es", "spa", "Spanish", "Español";
    "es-419", "spa", "Spanish (Latin America)", "Español latinoamericano";
    "sw", "swa", "Swahili", "Kiswahili";
    "sv", "swe", "Swedish", "Svenska";
    "ta", "tam", "Tamil", "தமிழ்";
    "te", "tel", "Telugu", "తెలుగు";
    "th", "tha", "Thai", "ไทย";
    "tr", "tur", "Turkish", "Türkçe";
    "uk", "ukr", "Ukrainian", "Українська";
    "ur", "urd", "Urdu", "اردو";
    "uz", "uzb", "Uzbek", "Oʻzbek";
    "vi", "vie", "Vietnamese", "Tiếng Việt";
    "cy", "wel", "Welsh", "Cymraeg";
    "yo", "yor", "Yoruba", "Yorùbá";
    "zu", "zul", "Zulu", "isiZulu";
};

pub fn all_languages() -> &'static [Language] {
    LANGUAGES
}

/// ISO 639-2 terminology codes that differ from the bibliographic ones in the table.
/// Media files use both spellings.
static ISO_639_2_TERMINOLOGY_ALIASES: &[(&str, &str)] = &[
    ("deu", "ger"), ("fra", "fre"), ("nld", "dut"), ("zho", "chi"), ("ces", "cze"), ("ell", "gre"),
    ("isl", "ice"), ("mkd", "mac"), ("msa", "may"), ("ron", "rum"), ("slk", "slo"), ("sqi", "alb"),
    ("hye", "arm"), ("eus", "baq"), ("mya", "bur"), ("kat", "geo"), ("fas", "per"), ("cym", "wel"),
    ("nob", "nor"), ("nno", "nor"),
];

/// Looks a language up by any of its codes (two-letter, three-letter, or a BCP 47
/// tag like `pt-BR`). Case-insensitive; a plain region-less match is tried last.
pub fn find_language(code: &str) -> Option<&'static Language> {
    let wanted = code.trim();
    if wanted.is_empty() {
        return None;
    }
    let wanted = ISO_639_2_TERMINOLOGY_ALIASES
        .iter()
        .find(|(terminology, _)| terminology.eq_ignore_ascii_case(wanted))
        .map(|(_, bibliographic)| *bibliographic)
        .unwrap_or(wanted);
    let exact = LANGUAGES
        .iter()
        .find(|l| l.code.eq_ignore_ascii_case(wanted) || l.code3.eq_ignore_ascii_case(wanted));
    if exact.is_some() {
        return exact;
    }
    let base = wanted.split(['-', '_']).next().unwrap_or(wanted);
    LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(base))
}

/// Looks a language up by its English or native name, the way subtitle files are often
/// named (`English.srt`). Case-insensitive.
pub fn find_language_by_name(name: &str) -> Option<&'static Language> {
    let wanted = name.trim().to_lowercase();
    LANGUAGES.iter().find(|l| l.english_name.to_lowercase() == wanted || l.native_name.to_lowercase() == wanted)
}

/// Human readable name for a code, falling back to the code itself.
pub fn language_name(code: &str) -> String {
    find_language(code).map(|l| l.english_name.to_string()).unwrap_or_else(|| code.to_string())
}

/// True when two codes denote the same language (`eng` == `en` == `en-US`).
pub fn same_language(a: &str, b: &str) -> bool {
    match (find_language(a), find_language(b)) {
        (Some(la), Some(lb)) => la.code == lb.code || (la.code3 == lb.code3 && !la.code.contains('-') && !lb.code.contains('-')),
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// The user's system language as one of the table's codes, defaulting to English.
pub fn system_language_code() -> String {
    sys_locale::get_locale()
        .and_then(|locale| {
            find_language(&locale)
                .or_else(|| find_language(locale.split(['-', '_']).next().unwrap_or(&locale)))
                .map(|l| l.code.to_string())
        })
        .unwrap_or_else(|| "en".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_any_code_form() {
        assert_eq!(find_language("de").unwrap().english_name, "German");
        assert_eq!(find_language("ger").unwrap().english_name, "German");
        assert_eq!(find_language("deu").unwrap().english_name, "German");
        assert_eq!(find_language("fra").unwrap().english_name, "French");
        assert_eq!(find_language("de-AT").unwrap().english_name, "German");
        assert_eq!(find_language("pt-BR").unwrap().english_name, "Portuguese (Brazil)");
        assert!(find_language("xx").is_none());
    }

    #[test]
    fn same_language_ignores_code_style() {
        assert!(same_language("eng", "en"));
        assert!(same_language("en-US", "en"));
        assert!(!same_language("de", "en"));
    }
}
