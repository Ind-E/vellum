use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FontConfig {
    family: Option<String>,
    weight: Option<f32>,
    style: Option<String>,
    #[serde(default)]
    features: BTreeMap<String, u16>,
}

impl FontConfig {
    pub(super) fn resolve(&self) -> Result<crate::text::TextFont, String> {
        use parley::setting::Tag;
        use parley::style::{FontFamilyName, FontFeature, FontStyle, FontWeight};

        let family = FontFamilyName::parse_css_list(self.family.as_deref().unwrap_or("sans-serif"))
            .map(|name| name.map(FontFamilyName::into_owned))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("invalid tools.text.font.family: {error}"))?;
        if family.is_empty() {
            return Err("tools.text.font.family must not be empty".into());
        }
        let weight = self.weight.unwrap_or(400.0);
        if !weight.is_finite() || !(1.0..=1000.0).contains(&weight) {
            return Err("tools.text.font.weight must be between 1 and 1000".into());
        }
        let style = match self.style.as_deref().unwrap_or("normal") {
            "normal" => FontStyle::Normal,
            "italic" => FontStyle::Italic,
            "oblique" => FontStyle::Oblique(Some(14.0)),
            _ => return Err("tools.text.font.style must be normal, italic, or oblique".into()),
        };
        let features = self
            .features
            .iter()
            .map(|(name, &value)| {
                let tag = Tag::parse(name).ok_or_else(|| {
                    format!(
                        "invalid tools.text.font.features tag {name:?}: expected four printable ASCII characters"
                    )
                })?;
                Ok(FontFeature::new(tag, value))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(crate::text::TextFont {
            family,
            weight: FontWeight::new(weight),
            style,
            features,
        })
    }
}
