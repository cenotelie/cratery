/*******************************************************************************
 * Copyright (c) 2024 Cénotélie Opérations SAS (cenotelie.fr)
 ******************************************************************************/

//! Data types around dependency analysis

use chrono::NaiveDateTime;
use log::error;
use semver::{Version, VersionReq};
use serde_derive::{Deserialize, Serialize};

use super::CrateVersion;
use super::cargo::{DependencyKind, IndexCrateDependency, IndexCrateMetadata};
use super::osv::SimpleAdvisory;
use crate::utils::apierror::ApiError;
use crate::utils::push_if_not_present;

/// The URI of the fake registry for built-in crates
pub const BUILTIN_CRATES_REGISTRY_URI: &str = "<builtin>";

/// The list of built-in crates
pub const BUILTIN_CRATES_LIST: &[&str] = &["core", "alloc", "std"];

/// The specification for a dependency analysis job
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DepsAnalysisJobSpec {
    /// The name of the crate
    pub package: String,
    /// The crate's version
    pub version: String,
    /// The targets for the crate
    pub targets: Vec<String>,
}

impl From<DepsAnalysisState> for DepsAnalysisJobSpec {
    fn from(state: DepsAnalysisState) -> Self {
        Self {
            package: state.package,
            version: state.version,
            targets: state.targets,
        }
    }
}

/// Metadata about a crate version and its analysis state
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DepsAnalysisState {
    /// The name of the crate
    pub package: String,
    /// The crate's version
    pub version: String,
    /// Whether the entire package is deprecated
    #[serde(rename = "isDeprecated")]
    pub is_deprecated: bool,
    /// Whether the version has outdated dependencies
    #[serde(rename = "depsHasOutdated")]
    pub deps_has_outdated: bool,
    /// When this crate version was last checked
    #[serde(rename = "depsLastCheck")]
    pub deps_last_check: NaiveDateTime,
    /// The targets associated with the crate
    pub targets: Vec<String>,
}

impl From<DepsAnalysisState> for CrateVersion {
    fn from(state: DepsAnalysisState) -> Self {
        Self {
            package: state.package,
            version: state.version,
        }
    }
}

/// The complete dependency analysis
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DepsAnalysis {
    /// The direct dependencies
    #[serde(rename = "directDependencies")]
    pub direct_dependencies: Vec<DirectDepInfo>,
    /// The advisories against dependencies
    pub advisories: Vec<DepAdvisory>,
}

impl DepsAnalysis {
    /// Creates the analysis
    #[must_use]
    pub fn new(graph: &DepsGraph, deps: &[IndexCrateDependency], advisories: Vec<DepAdvisory>) -> Self {
        Self {
            direct_dependencies: deps
                .iter()
                .zip(&graph.crates)
                .map(|(dep, data)| {
                    let resolved = data
                        .resolutions
                        .iter()
                        .find(|r| r.origins.contains(&DepsGraphCrateOrigin::Direct(dep.kind)));
                    let is_outdated = resolved.is_some_and(|res| data.versions[res.version_index].is_outdated);
                    DirectDepInfo {
                        registry: dep.registry.clone(),
                        package: dep.get_name().to_string(),
                        required: dep.req.clone(),
                        kind: dep.kind,
                        last_version: data.last_version.to_string(),
                        is_outdated,
                    }
                })
                .collect(),
            advisories,
        }
    }
}

/// The information about a direct dependency, resulting from an analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectDepInfo {
    /// URI for the owning registry, `None` for the local one
    pub registry: Option<String>,
    /// The name of the package
    pub package: String,
    /// The semver requirement for this dependency
    pub required: String,
    /// The kind of dependency
    pub kind: DependencyKind,
    /// The last known version
    #[serde(rename = "lastVersion")]
    pub last_version: String,
    /// Whether the requirement leads to the resolution of an outdated version
    #[serde(rename = "isOutdated")]
    pub is_outdated: bool,
}

/// The advisory against a dependency resolved on crates.io
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepAdvisory {
    /// The name of the package
    pub package: String,
    /// The resolved version
    pub version: Version,
    /// The advisory itself
    pub content: SimpleAdvisory,
}

impl IndexCrateMetadata {
    /// Assumes this is the metadata for a crate in an external registry, including crates.io
    /// Find and rewrite the registry for built-in crates
    #[must_use]
    pub fn rewrite_builtin_deps(mut self, parent_registry: &Option<String>) -> Self {
        for d in &mut self.deps {
            if d.registry.is_none() {
                if BUILTIN_CRATES_LIST.contains(&d.get_name()) {
                    d.registry = Some(String::from(BUILTIN_CRATES_REGISTRY_URI));
                } else {
                    d.registry.clone_from(parent_registry);
                }
            }
        }
        self
    }
}

/// A complete dependency graphs
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DepsGraph {
    /// The targets for the resolution
    pub targets: Vec<String>,
    /// All the crates in the graph
    pub crates: Vec<DepsGraphCrate>,
    /// The list of unknown crates that failed before
    pub unknowns: Vec<(Option<String>, String)>,
    /// The crate and resolution to analyse
    pub dirty: Vec<(usize, usize)>,
}

impl DepsGraph {
    /// Builds an empty graph for the specified targets
    #[must_use]
    pub fn new(targets: &[String]) -> Self {
        Self {
            targets: targets.to_vec(),
            ..Default::default()
        }
    }

    /// Gets the crate in the graph, if it exists
    pub fn get_crate(&mut self, registry: Option<&str>, name: &str) -> Option<(usize, &mut DepsGraphCrate)> {
        self.crates
            .iter_mut()
            .enumerate()
            .find(|(_, c)| c.registry.as_deref() == registry && c.name == name)
    }

    /// Gets whether this is a known failing crate
    #[must_use]
    pub fn is_unknown(&self, registry: Option<&str>, name: &str) -> bool {
        self.unknowns.iter().any(|(r, n)| r.as_deref() == registry && n == name)
    }

    /// Resolves a dependency within this graph
    pub async fn resolve<F, FUT>(
        &mut self,
        dep: &IndexCrateDependency,
        features: &[String],
        origins: &[DepsGraphCrateOrigin],
        get_versions: &F,
    ) -> Result<(), ApiError>
    where
        F: Fn(Option<String>, String) -> FUT,
        FUT: std::future::Future<Output = Result<Vec<IndexCrateMetadata>, ApiError>>,
    {
        if let Some((crate_index, c)) = self.get_crate(dep.registry.as_deref(), dep.get_name()) {
            if let Some(resolution_index) = c.resolve(dep, features, origins) {
                push_if_not_present(&mut self.dirty, (crate_index, resolution_index));
            }
        } else if !self.is_unknown(dep.registry.as_deref(), dep.get_name()) {
            let all_versions = match get_versions(dep.registry.clone(), dep.get_name().to_string()).await {
                Ok(d) => d,
                Err(e) => {
                    error!("deps: FAILED TO GET {:?} / {} => {}", dep.registry, dep.get_name(), e);
                    self.unknowns.push((dep.registry.clone(), dep.get_name().to_string()));
                    return Ok(());
                }
            };
            self.crates.push(DepsGraphCrate::new(dep, all_versions)?);
            let crate_index = self.crates.len() - 1;
            if let Some(resolution_index) = self.crates.last_mut().unwrap().resolve(dep, features, origins) {
                push_if_not_present(&mut self.dirty, (crate_index, resolution_index));
            }
        }
        Ok(())
    }

    /// Closes this graph
    ///
    /// Closes over the direct dependencies already in the graph.
    /// The direct dependencies include normal, dev and build dependencies
    pub async fn close<F, FUT>(&mut self, get_versions: &F) -> Result<(), ApiError>
    where
        F: Fn(Option<String>, String) -> FUT,
        FUT: std::future::Future<Output = Result<Vec<IndexCrateMetadata>, ApiError>>,
    {
        while let Some((crate_index, resolution_index)) = self.dirty.pop() {
            // new selected version/origin
            let dependencies = self.crates[crate_index]
                .get_active_deps_in(resolution_index, &self.targets)
                .map(|(dep, features)| (dep.clone(), features.into_iter().map(str::to_string).collect::<Vec<_>>()))
                .collect::<Vec<_>>();
            for (dep, features) in dependencies {
                let origins = self.crates[crate_index].resolutions[resolution_index]
                    .origins
                    .iter()
                    .filter_map(|&origin| origin.child_of_kind(dep.kind))
                    .collect::<Vec<_>>();
                if !origins.is_empty() {
                    self.resolve(&dep, &features, &origins, get_versions).await?;
                }
            }
        }
        Ok(())
    }
}

/// Reason why a requirement for a crate is in the closure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DepsGraphCrateOrigin {
    /// This is a direct dependency of a kind
    Direct(DependencyKind),
    /// This is an indirect dependency of a normal direct dependency
    NormalIndirect,
    /// This is an indirect dependency required for the build
    BuildIndirect,
    /// The indirect normal dependency of a dev direct dependency
    DevNormalIndirect,
    /// The indirect build dependency of a dev direct dependency
    DevBuildIndirect,
}

impl DepsGraphCrateOrigin {
    /// Gets the origin for a sub-dependency of a specified kind with a dependant of the current origin
    #[allow(clippy::match_same_arms)]
    #[must_use]
    pub fn child_of_kind(self, kind: DependencyKind) -> Option<DepsGraphCrateOrigin> {
        match (self, kind) {
            (_, DependencyKind::Dev) => None, // drop all other dev-dependencies
            (Self::Direct(DependencyKind::Normal), DependencyKind::Normal) => Some(Self::NormalIndirect),
            (Self::Direct(DependencyKind::Normal), DependencyKind::Build) => Some(Self::BuildIndirect),
            (Self::Direct(DependencyKind::Build), DependencyKind::Normal) => Some(Self::BuildIndirect),
            (Self::Direct(DependencyKind::Build), DependencyKind::Build) => Some(Self::BuildIndirect),
            (Self::Direct(DependencyKind::Dev), DependencyKind::Normal) => Some(Self::DevNormalIndirect),
            (Self::Direct(DependencyKind::Dev), DependencyKind::Build) => Some(Self::DevBuildIndirect),
            (Self::NormalIndirect, DependencyKind::Normal) => Some(Self::NormalIndirect),
            (Self::BuildIndirect, DependencyKind::Normal) => Some(Self::BuildIndirect),
            (Self::NormalIndirect, DependencyKind::Build) => Some(Self::BuildIndirect),
            (Self::BuildIndirect, DependencyKind::Build) => Some(Self::BuildIndirect),
            (Self::DevNormalIndirect, DependencyKind::Normal) => Some(Self::DevNormalIndirect),
            (Self::DevNormalIndirect, DependencyKind::Build) => Some(Self::DevBuildIndirect),
            (Self::DevBuildIndirect, DependencyKind::Normal) => Some(Self::DevBuildIndirect),
            (Self::DevBuildIndirect, DependencyKind::Build) => Some(Self::DevBuildIndirect),
        }
    }
}

/// The known version of a crate, coming from the index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepsGraphCrateVersion {
    /// The semver version
    pub semver: Version,
    /// The index metadata
    pub metadata: IndexCrateMetadata,
    /// Whether this version is outdated
    #[serde(rename = "isOutdated")]
    pub is_outdated: bool,
}

/// The resolution of a crate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepsGraphCrateResolution {
    /// The index of the version in the list of all versions
    #[serde(rename = "versionIndex")]
    pub version_index: usize,
    /// Whether the default features are activated
    #[serde(rename = "defaultFeatures")]
    pub default_features: bool,
    /// The activated features if any
    pub features: Vec<String>,
    /// The origins for this activation
    pub origins: Vec<DepsGraphCrateOrigin>,
}

/// A crate in a graph of dependencies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepsGraphCrate {
    /// URI for the owning registry, `None` for the local one
    pub registry: Option<String>,
    /// The name of the package
    pub name: String,
    /// All the known versions
    pub versions: Vec<DepsGraphCrateVersion>,
    /// The version number of the latest stable version
    #[serde(rename = "lastVersion")]
    pub last_version: Version,
    /// The resolved versions of this crate, actually appearing in the dependency graph
    pub resolutions: Vec<DepsGraphCrateResolution>,
    /// The list of unresolved requirements for this crate
    pub unresolved: Vec<VersionReq>,
}

impl DepsGraphCrate {
    /// Creates the data for this crate
    pub fn new(package: &IndexCrateDependency, versions: Vec<IndexCrateMetadata>) -> Result<Self, semver::Error> {
        let last_version = versions
            .iter()
            // filter out yanked and pre- versions
            .filter_map(|metadata| {
                if metadata.yanked {
                    None
                } else if let Ok(vers) = metadata.vers.parse::<Version>() {
                    if vers.pre.is_empty() { Some(vers) } else { None }
                } else {
                    None
                }
            })
            .max()
            .unwrap();
        let versions = versions
            .into_iter()
            .map(|metadata| {
                let semver = metadata.vers.parse::<Version>()?;
                Ok(DepsGraphCrateVersion {
                    is_outdated: semver < last_version,
                    semver,
                    metadata: metadata.rewrite_builtin_deps(&package.registry),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            registry: package.registry.clone(),
            name: package.get_name().to_string(),
            versions,
            last_version,
            resolutions: Vec::new(),
            unresolved: Vec::new(),
        })
    }

    /// Resolves a version of this crate for the specified dependency
    /// If this leads to modifications, yield the index of the corresponding resolution to (re-)visit
    pub fn resolve(
        &mut self,
        dep: &IndexCrateDependency,
        features: &[String],
        origins: &[DepsGraphCrateOrigin],
    ) -> Option<usize> {
        let semver = dep.req.parse::<VersionReq>().unwrap();
        let version_index = self
            .versions
            .iter()
            .enumerate()
            .filter(|(_, version)| semver.matches(&version.semver))
            .max_by(|(_, v1), (_, v2)| v1.semver.cmp(&v2.semver))
            .map(|(i, _)| i);
        let Some(version_index) = version_index else {
            self.unresolved.push(semver);
            return None;
        };

        if let Some((resolution_index, resolution)) = self
            .resolutions
            .iter_mut()
            .enumerate()
            .find(|(_, res)| res.version_index == version_index)
        {
            let mut modified = false;
            for feature in features {
                modified |= push_if_not_present(&mut resolution.features, feature.clone());
            }
            for &origin in origins {
                modified |= push_if_not_present(&mut resolution.origins, origin);
            }
            if modified { Some(resolution_index) } else { None }
        } else {
            let mut resolution = DepsGraphCrateResolution {
                version_index,
                default_features: dep.default_features,
                features: dep.features.clone(),
                origins: origins.to_vec(),
            };
            for feature in features {
                push_if_not_present(&mut resolution.features, feature.clone());
            }
            self.resolutions.push(resolution);
            Some(self.resolutions.len() - 1)
        }
    }

    /// Gets the active dependencies for a resolution
    pub fn get_active_deps_in<'this: 'targets, 'targets>(
        &'this self,
        resolution_index: usize,
        targets: &'targets [String],
    ) -> impl Iterator<Item = (&'this IndexCrateDependency, Vec<&'this str>)> + 'targets {
        let resolution = &self.resolutions[resolution_index];
        let version = &self.versions[resolution.version_index];
        let active_features = Self::get_active_features(resolution, version);
        version.metadata.deps.iter().filter_map(move |dep| {
            let is_active = dep.is_active_for(targets, &active_features);
            if is_active {
                let sub_features = active_features
                    .iter()
                    .filter_map(|feature| {
                        let index = feature.find('/')?;
                        if &feature[..index] == dep.get_name()
                            || (feature[..index].ends_with('?') && &feature[..(index - 1)] == dep.get_name())
                        {
                            Some(&feature[(index + 1)..])
                        } else {
                            None
                        }
                    })
                    .collect();
                Some((dep, sub_features))
            } else {
                None
            }
        })
    }

    /// Gets the full list of activated features for a resolution of this crate
    fn get_active_features<'a>(resolution: &'a DepsGraphCrateResolution, version: &'a DepsGraphCrateVersion) -> Vec<&'a str> {
        let mut active_features = Vec::new();
        if resolution.default_features {
            if let Some(children) = version.metadata.get_feature("default") {
                active_features.push("default");
                for f in children {
                    push_if_not_present(&mut active_features, f.as_str());
                }
            }
        }
        for feature in &resolution.features {
            push_if_not_present(&mut active_features, feature.as_str());
            if let Some(children) = version.metadata.get_feature(feature) {
                for f in children {
                    push_if_not_present(&mut active_features, f.as_str());
                }
            }
        }
        // close
        let mut index = 0;
        while index < active_features.len() {
            if let Some(children) = version.metadata.get_feature(active_features[index]) {
                for f in children {
                    push_if_not_present(&mut active_features, f.as_str());
                }
            }
            index += 1;
        }
        active_features
    }
}
