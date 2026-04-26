use std::collections::HashMap;

use anyhow::{anyhow, Result};
use fluent::concurrent::FluentBundle;
use fluent::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

#[derive(Debug, Clone, Copy)]
pub struct LanguageOption {
    pub code: &'static str,
    pub name: &'static str,
}

pub const SUPPORTED_LANGUAGES: &[LanguageOption] = &[
    LanguageOption {
        code: "tr",
        name: "Turkce",
    },
    LanguageOption {
        code: "en",
        name: "English",
    },
    LanguageOption {
        code: "es",
        name: "Espanol",
    },
    LanguageOption {
        code: "it",
        name: "Italiano",
    },
    LanguageOption {
        code: "ru",
        name: "Русский",
    },
    LanguageOption {
        code: "fr",
        name: "Francais",
    },
    LanguageOption {
        code: "de",
        name: "Deutsch",
    },
    LanguageOption {
        code: "mi",
        name: "Te Reo Maori",
    },
];

pub struct I18n {
    bundles: HashMap<String, FluentBundle<FluentResource>>,
    fallback: String,
}

impl I18n {
    pub fn new() -> Result<Self> {
        let resources = [
            ("tr", include_str!("../locales/tr.ftl")),
            ("en", include_str!("../locales/en.ftl")),
            ("es", include_str!("../locales/es.ftl")),
            ("it", include_str!("../locales/it.ftl")),
            ("ru", include_str!("../locales/ru.ftl")),
            ("fr", include_str!("../locales/fr.ftl")),
            ("de", include_str!("../locales/de.ftl")),
            ("mi", include_str!("../locales/mi.ftl")),
        ];

        let mut bundles = HashMap::new();
        for (code, source) in resources {
            let lang_id: LanguageIdentifier = code.parse()?;
            let resource = FluentResource::try_new(source.to_string())
                .map_err(|(_, errors)| anyhow!("failed to parse {code}.ftl: {errors:?}"))?;
            let mut bundle = FluentBundle::new_concurrent(vec![lang_id]);
            bundle
                .add_resource(resource)
                .map_err(|errors| anyhow!("failed to add {code}.ftl: {errors:?}"))?;
            bundles.insert(code.to_string(), bundle);
        }

        Ok(Self {
            bundles,
            fallback: "en".to_string(),
        })
    }

    pub fn normalize_language(&self, language_code: Option<&str>) -> String {
        let Some(language_code) = language_code else {
            return self.fallback.clone();
        };

        let primary = language_code
            .split(['-', '_'])
            .next()
            .unwrap_or(language_code)
            .to_ascii_lowercase();

        if self.bundles.contains_key(&primary) {
            primary
        } else {
            self.fallback.clone()
        }
    }

    pub fn is_supported(&self, language_code: &str) -> bool {
        self.bundles.contains_key(language_code)
    }

    pub fn language_name(&self, language_code: &str) -> &'static str {
        SUPPORTED_LANGUAGES
            .iter()
            .find(|language| language.code == language_code)
            .map(|language| language.name)
            .unwrap_or("English")
    }

    pub fn t(&self, language_code: &str, key: &str, args: &[(&str, String)]) -> String {
        let language_code = if self.bundles.contains_key(language_code) {
            language_code
        } else {
            self.fallback.as_str()
        };

        self.format(language_code, key, args)
            .or_else(|| self.format(&self.fallback, key, args))
            .unwrap_or_else(|| key.to_string())
    }

    fn format(&self, language_code: &str, key: &str, args: &[(&str, String)]) -> Option<String> {
        let bundle = self.bundles.get(language_code)?;
        let message = bundle.get_message(key)?;
        let pattern = message.value()?;

        let mut fluent_args = FluentArgs::new();
        for (name, value) in args {
            fluent_args.set(*name, FluentValue::from(value.as_str()));
        }

        let mut errors = Vec::new();
        let value = if args.is_empty() {
            bundle.format_pattern(pattern, None, &mut errors)
        } else {
            bundle.format_pattern(pattern, Some(&fluent_args), &mut errors)
        };

        if !errors.is_empty() {
            log::warn!("fluent formatting errors for {language_code}/{key}: {errors:?}");
        }

        Some(value.into_owned())
    }
}
