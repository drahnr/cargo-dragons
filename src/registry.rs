//! Registry selection shared by release planning, authentication, and publishing.

use anyhow::{Context, bail};
use cargo::{
    GlobalContext,
    core::{Package, SourceId},
    ops::RegistryOrIndex,
    sources::SourceConfigMap,
    util::auth,
};
use cargo_credential::{Operation, Secret};
use std::collections::{BTreeMap, BTreeSet};

/// Command-line credentials resolved once against the selected publication registries.
#[derive(Default)]
pub(crate) struct RegistryTokens(BTreeMap<String, Secret<String>>);

impl RegistryTokens {
    pub(crate) fn from_cli<'a>(
        tokens: Vec<String>,
        registry_names: impl IntoIterator<Item = &'a str>,
    ) -> anyhow::Result<Self> {
        let names = registry_names.into_iter().collect::<BTreeSet<_>>();
        let mut resolved = BTreeMap::new();
        for value in tokens {
            let (name, token) = match value.split_once(':') {
                Some((name, token)) => {
                    anyhow::ensure!(!name.is_empty(), "--token registry name must not be empty");
                    (name, token)
                }
                None if names.len() <= 1 => (
                    names.first().copied().unwrap_or("crates-io"),
                    value.as_str(),
                ),
                None => anyhow::bail!(
                    "An unprefixed --token requires exactly one selected publication registry; \
                     specify the registry explicitly with --token REGISTRY:TOKEN"
                ),
            };
            anyhow::ensure!(
                !token.is_empty(),
                "--token for registry `{name}` must not be empty"
            );
            anyhow::ensure!(
                names.contains(name),
                "--token names registry `{name}`, which is not a selected publication registry. \
                 Use a configured registry selected by package.publish."
            );
            anyhow::ensure!(
                !resolved.contains_key(name),
                "Multiple --token values supplied for registry `{name}`; provide only one"
            );
            resolved.insert(name.to_owned(), Secret::from(token.to_owned()));
        }
        Ok(Self(resolved))
    }

    pub(crate) fn for_registry(&self, registry: &PublicationRegistry) -> Option<&Secret<String>> {
        self.0.get(registry.name())
    }

    /// Validate every target before any index queries, uploads, or owner changes.
    pub(crate) fn ensure_logins<'a>(
        &self,
        gctx: &GlobalContext,
        registries: impl IntoIterator<Item = &'a PublicationRegistry>,
    ) -> anyhow::Result<()> {
        for registry in registries {
            gctx.shell()
                .status("Checking", format!("{} login", registry.name()))?;
            registry.ensure_login(gctx, self.for_registry(registry))?;
        }
        Ok(())
    }
}

/// The unambiguous named registry to which a package may be published.
#[derive(Clone, Debug)]
pub(crate) struct PublicationRegistry {
    name: String,
    source_id: SourceId,
    index_source_id: SourceId,
}

impl PublicationRegistry {
    /// Resolve the per-package target using Cargo's publication inference rules.
    pub(crate) fn for_package(gctx: &GlobalContext, package: &Package) -> anyhow::Result<Self> {
        let name = match package.publish().as_deref() {
            None => "crates-io",
            Some([name]) => name.as_str(),
            Some([]) => bail!(
                "{} is marked as publish = false; refusing to release it",
                package.name()
            ),
            Some(names) => bail!(
                "Package `{}` has an ambiguous publication registry allowlist: {:?}. \
                 Set `package.publish` in its Cargo.toml to a single registry; \
                 cargo dragons does not support a --registry override.",
                package.name(),
                names
            ),
        };
        let source_id = if name == "crates-io" {
            SourceId::crates_io(gctx)
        } else {
            SourceId::alt_registry(gctx, name)
        }
        .with_context(|| {
            format!(
                "Could not resolve publication registry `{name}` for package `{}`",
                package.name()
            )
        })?;

        // Match Cargo's built-in crates.io protocol mapping, but never silently
        // bypass its refusal to publish an implicit target with source replacement.
        let index_source_id = SourceConfigMap::empty(gctx)?
            .load(source_id, &Default::default())?
            .replaced_source_id();
        if package.publish().is_none() {
            let configured_source_id = SourceConfigMap::new(gctx)?
                .load(source_id, &Default::default())?
                .replaced_source_id();
            anyhow::ensure!(
                configured_source_id == index_source_id,
                "crates-io is replaced with {configured_source_id}; refusing implicit publication \
                 of `{}`. Set `package.publish` to a single explicit registry before releasing.",
                package.name()
            );
        }

        Ok(Self {
            name: name.to_owned(),
            source_id,
            index_source_id,
        })
    }

    /// The Cargo registry name, including `crates-io` for the default registry.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The source used to query publication state and obtain credentials.
    pub(crate) fn source_id(&self) -> SourceId {
        self.source_id
    }

    /// The built-in registry source used for index access, without user source replacement.
    pub(crate) fn index_source_id(&self) -> SourceId {
        self.index_source_id
    }

    /// An explicit named registry for Cargo's publishing and owner operations.
    pub(crate) fn reg_or_index(&self) -> RegistryOrIndex {
        RegistryOrIndex::Registry(self.name().to_owned())
    }

    /// Check Cargo credentials, optionally caching an explicit token for this target only.
    pub(crate) fn ensure_login(
        &self,
        gctx: &GlobalContext,
        token: Option<&Secret<String>>,
    ) -> anyhow::Result<()> {
        let source_id = self.source_id();
        if let Some(token) = token {
            // Cargo's cache is process-local; this does not update credentials.toml.
            auth::cache_token_from_commandline(gctx, &source_id, Secret::as_deref(token));
        }

        // Let Cargo select this registry's credential provider and environment token.
        // In particular, do not read or forward CARGO_REGISTRY_TOKEN ourselves.
        auth::auth_token(gctx, &source_id, None, Operation::Read, Vec::new(), false)
            .map(|_| ())
            .with_context(|| {
                if self.name() == "crates-io" {
                    "Not logged in to crates.io. Run `cargo login` \
                     (`cargo login --registry crates-io`) or provide a token with \
                     `--token`/`CARGO_REGISTRY_TOKEN` before running `cargo dragons unleash` \
                     (including `--dry-run`)."
                        .to_owned()
                } else {
                    format!(
                        "Not logged in to registry `{}`. Run `cargo login --registry {}` \
                         or provide a token with `--token {}:TOKEN`/`{}` before running \
                         `cargo dragons unleash` (including `--dry-run`).",
                        self.name(),
                        self.name(),
                        self.name(),
                        token_environment_variable(self.name())
                    )
                }
            })
    }
}

fn token_environment_variable(name: &str) -> String {
    if name == "crates-io" {
        "CARGO_REGISTRY_TOKEN".to_owned()
    } else {
        format!(
            "CARGO_REGISTRIES_{}_TOKEN",
            name.to_ascii_uppercase().replace('-', "_")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{RegistryTokens, token_environment_variable};

    #[test]
    fn bare_token_targets_crates_io_in_a_mixed_selection() -> anyhow::Result<()> {
        let tokens = RegistryTokens::from_cli(
            vec!["public-token".to_owned(), "fff-rs:private-token".to_owned()],
            ["crates-io", "fff-rs"],
        )?;
        assert_eq!(
            tokens.0.keys().map(String::as_str).collect::<Vec<_>>(),
            ["crates-io", "fff-rs"]
        );
        Ok(())
    }

    #[test]
    fn bare_token_targets_the_single_registry_even_with_multiple_packages() -> anyhow::Result<()> {
        let tokens =
            RegistryTokens::from_cli(vec!["private-token".to_owned()], ["fff-rs", "fff-rs"])?;
        assert_eq!(
            tokens.0.keys().map(String::as_str).collect::<Vec<_>>(),
            ["fff-rs"]
        );
        Ok(())
    }

    #[test]
    fn bare_token_cannot_choose_between_alternate_registries() {
        let result =
            RegistryTokens::from_cli(vec!["private-token".to_owned()], ["fff-rs", "other"]);
        let error = result
            .err()
            .expect("a bare token requires a single registry or crates-io");
        assert!(error.to_string().contains("crates-io"));
        assert!(!error.to_string().contains("private-token"));
    }

    #[test]
    fn token_environment_variables_are_registry_specific() {
        assert_eq!(
            token_environment_variable("crates-io"),
            "CARGO_REGISTRY_TOKEN"
        );
        assert_eq!(
            token_environment_variable("my-registry"),
            "CARGO_REGISTRIES_MY_REGISTRY_TOKEN"
        );
        assert_eq!(
            token_environment_variable("internal_registry"),
            "CARGO_REGISTRIES_INTERNAL_REGISTRY_TOKEN"
        );
    }
}
