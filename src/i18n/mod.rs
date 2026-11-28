//! Centralized bilingual (Chinese / English) UI copy and active locale state.
pub mod ai;
pub mod canvas;
pub mod common;
pub mod graph;
pub mod markdown;
pub mod media;
pub mod preferences;
pub mod right_panel;
pub mod tags;
pub mod workspace;

use crate::model::AppSettings;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Locale {
    #[default]
    ZhCn,
    En,
}

impl Locale {
    pub const ALL: [Self; 2] = [Self::ZhCn, Self::En];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ZhCn => "简体中文",
            Self::En => "English",
        }
    }

    const fn as_u8(self) -> u8 {
        match self {
            Self::ZhCn => 0,
            Self::En => 1,
        }
    }

    const fn from_u8(raw: u8) -> Self {
        match raw {
            1 => Self::En,
            _ => Self::ZhCn,
        }
    }
}

static ACTIVE_LOCALE: AtomicU8 = AtomicU8::new(0);

pub fn locale() -> Locale {
    Locale::from_u8(ACTIVE_LOCALE.load(Ordering::Relaxed))
}

pub fn set_locale(locale: Locale) {
    ACTIVE_LOCALE.store(locale.as_u8(), Ordering::Relaxed);
}

pub fn apply_settings(settings: &AppSettings) {
    set_locale(settings.locale);
}

/// Compile-time checked bilingual static string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BiText {
    pub zh: &'static str,
    pub en: &'static str,
}

impl BiText {
    pub const fn new(zh: &'static str, en: &'static str) -> Self {
        Self { zh, en }
    }

    pub const fn for_locale(self, locale: Locale) -> &'static str {
        match locale {
            Locale::ZhCn => self.zh,
            Locale::En => self.en,
        }
    }

    pub fn get(self) -> &'static str {
        self.for_locale(locale())
    }
}

impl From<BiText> for &'static str {
    fn from(value: BiText) -> Self {
        value.get()
    }
}

impl From<BiText> for gpui_kit::SharedString {
    fn from(value: BiText) -> Self {
        value.get().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitext_and_locale_serde_round_trip() {
        let copy = BiText::new("设置", "Settings");
        assert_eq!(copy.for_locale(Locale::ZhCn), "设置");
        assert_eq!(copy.for_locale(Locale::En), "Settings");
        assert_eq!(Locale::default(), Locale::ZhCn);

        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.locale, Locale::ZhCn);

        let mut en_settings = AppSettings::default();
        en_settings.locale = Locale::En;
        let json = serde_json::to_string(&en_settings).unwrap();
        let decoded: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.locale, Locale::En);
    }
}
