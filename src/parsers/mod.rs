//! Parsers for various task runner config file formats

mod cargo_toml;
mod csproj;
mod deno_json;
mod justfile;
mod makefile;
mod package_json;
mod pom_xml;
mod pubspec_yaml;
mod pyproject_toml;
mod turbo_json;

pub use cargo_toml::CargoTomlParser;
pub use csproj::CsprojParser;
pub use deno_json::DenoJsonParser;
pub use justfile::JustfileParser;
pub use makefile::MakefileParser;
pub use package_json::PackageJsonParser;
pub use pom_xml::PomXmlParser;
pub use pubspec_yaml::PubspecYamlParser;
pub use pyproject_toml::PyprojectTomlParser;
pub use turbo_json::TurboJsonParser;

use std::path::Path;

use crate::{ScanError, TaskRunner};

/// Trait for parsing task runner config files
pub trait Parser {
    /// Parse a config file and return a TaskRunner if tasks are found
    ///
    /// Returns Ok(None) if the file doesn't contain any tasks
    /// Returns Err if the file couldn't be parsed
    fn parse(&self, path: &Path) -> Result<Option<TaskRunner>, ScanError>;
}
