use std::{collections::BTreeSet, rc::Rc};

use cargo::core::{FeatureValue, resolver::CliFeatures as CargoCliFeatures};

/// Feature selectors parsed by [`Self::from_command_line`] using Cargo's CLI parser.
///
/// Input strings are split on commas and whitespace, checked for CLI feature syntax,
/// and deduplicated. Package defaults are not added, and feature implications are
/// not resolved at this stage.
pub(crate) struct CliFeatures {
    values: BTreeSet<FeatureValue>,
}

impl CliFeatures {
    pub(crate) fn from_command_line(features: &[String]) -> anyhow::Result<Self> {
        let request = CargoCliFeatures::from_command_line(features, false, false)?;
        Ok(Self {
            values: Rc::unwrap_or_clone(request.features),
        })
    }

    pub(crate) fn with_default(self, defaults: Option<&Vec<FeatureValue>>) -> Features {
        let mut features = self.without_defaults();
        if let Some(defaults) = defaults {
            let values = &mut features.values;
            // Keep the group itself for cfg(feature = "default") and dependency selectors
            // such as dep:foo or foo?/bar, which must not be passed as CLI features.
            values.insert(FeatureValue::Feature("default".into()));
            values.extend(
                defaults
                    .iter()
                    .filter(|value| matches!(value, FeatureValue::Feature(_)))
                    .cloned(),
            );
        }
        features
    }

    pub(crate) fn without_defaults(self) -> Features {
        Features {
            values: self.values,
        }
    }
}

/// A package's explicit feature selection, after choosing its default-feature policy.
///
/// Feature implications and dependency features are still resolved by Cargo.
#[derive(Clone, Debug)]
pub(crate) struct Features {
    values: BTreeSet<FeatureValue>,
}

impl Features {
    pub(crate) fn to_cargo(&self) -> CargoCliFeatures {
        CargoCliFeatures {
            features: Rc::new(self.values.clone()),
            all_features: false,
            uses_default_features: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo::core::FeatureMap;

    fn definitions() -> FeatureMap {
        FeatureMap::from_iter(
            [
                (
                    "default".into(),
                    vec![FeatureValue::Feature("backend-a".into())],
                ),
                ("backend-a".into(), vec![]),
                ("backend-b".into(), vec![]),
            ]
            .into_iter(),
        )
    }

    fn consolidated(
        requested: &[&str],
        uses_default_features: bool,
        definitions: &FeatureMap,
    ) -> anyhow::Result<Vec<String>> {
        let requested = Vec::from_iter(requested.iter().map(|s| s.to_string()));
        let cli = CliFeatures::from_command_line(&requested)?;
        let features = if uses_default_features {
            cli.with_default(definitions.get("default"))
        } else {
            cli.without_defaults()
        };
        let cargo_features = features.to_cargo();
        assert!(!cargo_features.all_features);
        assert!(!cargo_features.uses_default_features);
        Ok(Vec::from_iter(
            cargo_features.features.iter().map(ToString::to_string),
        ))
    }

    #[test]
    fn with_default_adds_declared_features_and_the_default_group() -> anyhow::Result<()> {
        assert_eq!(
            consolidated(&[], true, &definitions())?,
            ["backend-a", "default"]
        );
        Ok(())
    }

    #[test]
    fn without_defaults_keeps_the_requested_combination_isolated() -> anyhow::Result<()> {
        assert_eq!(
            consolidated(&["backend-b"], false, &definitions())?,
            ["backend-b"]
        );
        assert!(consolidated(&[], false, &definitions())?.is_empty());
        Ok(())
    }

    #[test]
    fn without_defaults_preserves_an_explicit_default() -> anyhow::Result<()> {
        assert_eq!(
            consolidated(&["default"], false, &definitions())?,
            ["default"]
        );
        Ok(())
    }

    #[test]
    fn with_default_merges_and_deduplicates_requested_features() -> anyhow::Result<()> {
        assert_eq!(
            consolidated(
                &["backend-b,backend-a", "backend-b", "default"],
                true,
                &definitions()
            )?,
            ["backend-a", "backend-b", "default"]
        );
        Ok(())
    }

    #[test]
    fn without_defaults_preserves_an_explicit_all_feature_combination() -> anyhow::Result<()> {
        assert_eq!(
            consolidated(
                &["backend-a", "backend-b", "default"],
                false,
                &definitions()
            )?,
            ["backend-a", "backend-b", "default"]
        );
        Ok(())
    }

    #[test]
    fn with_default_does_not_invent_an_undeclared_default() -> anyhow::Result<()> {
        assert!(consolidated(&[], true, &FeatureMap::new())?.is_empty());
        Ok(())
    }

    #[test]
    fn with_default_preserves_an_empty_default_group() -> anyhow::Result<()> {
        let definitions = FeatureMap::from_iter([("default".into(), vec![])].into_iter());
        assert_eq!(consolidated(&[], true, &definitions)?, ["default"]);
        Ok(())
    }

    #[test]
    fn with_default_leaves_dependency_selectors_to_the_default_group() -> anyhow::Result<()> {
        let mut definitions = definitions();
        definitions.get_mut("default").unwrap().extend([
            FeatureValue::new("dep:helper".into()),
            FeatureValue::new("helper?/extra".into()),
            FeatureValue::new("helper/other".into()),
        ]);
        assert_eq!(
            consolidated(&[], true, &definitions)?,
            ["backend-a", "default"]
        );
        Ok(())
    }

    #[test]
    fn parser_rejects_invalid_feature_syntax() {
        assert!(CliFeatures::from_command_line(&["dep:backend".into()]).is_err());
    }
}
