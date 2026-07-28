use crate::impact_context::cache::file_facts::{
    create_private_directory, is_symlink_or_reparse, open_regular_file_no_follow,
    set_private_file_permissions, sync_directory, CacheError, CacheLayout, CacheLookup,
    PublishResult,
};
use crate::impact_context::cache::sqlite_generation::{ReaderLimits, RepositoryGraphReader};
use crate::impact_context::contracts::Completeness;
use crate::impact_context::index::model::{GraphGenerationIdentity, RepositoryLocator};
use crate::review_scope::ReviewSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const REFERENCE_MAGIC: &str = "pre-commit-review-generation-reference";
const REFERENCE_SCHEMA_VERSION: u16 = 1;
const MAXIMUM_REFERENCE_BYTES: usize = 64 * 1024;
const MAXIMUM_REFERENCES_PER_LOCATOR: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationCompatibility {
    pub graph_schema_version: u16,
    pub resolver_digest: String,
    pub adapter_query_digest: String,
    pub normalization_rules_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationReference {
    lookup: GenerationLookup,
    pub locator: RepositoryLocator,
    pub compatibility: GenerationCompatibility,
    pub identity: GraphGenerationIdentity,
    pub generation_key: String,
    pub completeness: Completeness,
    pub manifest_files: usize,
    pub manifest_bytes: u64,
}

pub struct LocatedGeneration {
    pub reference: GenerationReference,
    pub reader: RepositoryGraphReader,
}

pub(crate) struct GenerationPublishOutcome {
    pub(crate) result: PublishResult,
    pub(crate) published_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationReferenceEnvelope {
    magic: String,
    schema_version: u16,
    payload_length: usize,
    payload_sha256: String,
    payload: GenerationReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum GenerationLookup {
    Exact {
        locator: RepositoryLocator,
    },
    BaseTree {
        object_format: String,
        tree: String,
    },
    IndexManifest {
        object_format: String,
        index_manifest_digest: String,
    },
}

#[derive(Debug, Clone)]
pub struct GenerationLocatorStore {
    layout: CacheLayout,
}

impl GenerationCompatibility {
    pub fn validate(&self) -> Result<(), CacheError> {
        if self.graph_schema_version == 0
            || !valid_sha256(&self.resolver_digest)
            || !valid_sha256(&self.adapter_query_digest)
            || !valid_sha256(&self.normalization_rules_digest)
        {
            return Err(CacheError::new(
                "generation-reference-compatibility-invalid",
                "generation reference compatibility is invalid",
            ));
        }
        Ok(())
    }
}

impl GenerationLocatorStore {
    pub fn new(layout: CacheLayout) -> Self {
        Self { layout }
    }

    pub fn lookup_exact(
        &self,
        locator: &RepositoryLocator,
        compatibility: &GenerationCompatibility,
        reader_limits: ReaderLimits,
    ) -> Result<CacheLookup<LocatedGeneration>, CacheError> {
        validate_lookup(locator, compatibility)?;
        self.lookup(
            &GenerationLookup::Exact {
                locator: locator.clone(),
            },
            compatibility,
            reader_limits,
        )
    }

    pub fn lookup_base(
        &self,
        locator: &RepositoryLocator,
        compatibility: &GenerationCompatibility,
        reader_limits: ReaderLimits,
    ) -> Result<CacheLookup<LocatedGeneration>, CacheError> {
        validate_lookup(locator, compatibility)?;
        let lookup = match locator.source {
            ReviewSource::Staged => GenerationLookup::BaseTree {
                object_format: locator.object_format.clone(),
                tree: locator.base_tree.clone().ok_or_else(|| {
                    CacheError::new(
                        "generation-reference-base-tree-missing",
                        "staged locator has no base tree",
                    )
                })?,
            },
            ReviewSource::Unstaged => GenerationLookup::IndexManifest {
                object_format: locator.object_format.clone(),
                index_manifest_digest: locator.index_manifest_digest.clone().ok_or_else(|| {
                    CacheError::new(
                        "generation-reference-index-manifest-missing",
                        "unstaged locator has no index manifest digest",
                    )
                })?,
            },
            ReviewSource::Branch => return Ok(CacheLookup::Miss),
        };
        self.lookup(&lookup, compatibility, reader_limits)
    }

    fn lookup(
        &self,
        lookup: &GenerationLookup,
        compatibility: &GenerationCompatibility,
        reader_limits: ReaderLimits,
    ) -> Result<CacheLookup<LocatedGeneration>, CacheError> {
        let bucket = self.bucket(lookup, compatibility)?;
        match validate_directory_chain(&self.layout.root, &bucket)? {
            CacheLookup::Hit(()) => {}
            CacheLookup::Miss => return Ok(CacheLookup::Miss),
            CacheLookup::Stale { code } => return Ok(CacheLookup::Stale { code }),
            CacheLookup::Corrupt { code } => return Ok(CacheLookup::Corrupt { code }),
        }
        let entries = fs::read_dir(&bucket).map_err(|error| {
            CacheError::new(
                "generation-reference-directory-unavailable",
                format!("cannot read generation reference directory: {error}"),
            )
        })?;
        let mut paths = Vec::with_capacity(MAXIMUM_REFERENCES_PER_LOCATOR);
        for entry in entries {
            let path = entry
                .map_err(|error| {
                    CacheError::new(
                        "generation-reference-directory-unavailable",
                        format!("cannot enumerate generation references: {error}"),
                    )
                })?
                .path();
            if paths.len() == MAXIMUM_REFERENCES_PER_LOCATOR {
                return Ok(corrupt("generation-reference-cardinality-exceeded"));
            }
            paths.push(path);
        }
        paths.sort();

        let mut stale_code = None;
        let mut corrupt_code = None;
        let mut candidates = Vec::new();
        for path in paths {
            match self.read_reference(&path, lookup, compatibility)? {
                CacheLookup::Hit(reference) => candidates.push(reference),
                CacheLookup::Miss => {}
                CacheLookup::Stale { code } => stale_code = Some(code),
                CacheLookup::Corrupt { code } => corrupt_code = Some(code),
            }
        }
        candidates.sort_by(|left, right| {
            completeness_rank(right.completeness)
                .cmp(&completeness_rank(left.completeness))
                .then_with(|| left.generation_key.cmp(&right.generation_key))
        });
        for reference in candidates {
            let path = self
                .layout
                .graphs_dir
                .join(format!("{}.sqlite", reference.generation_key));
            match RepositoryGraphReader::open_immutable(&path, &reference.identity, reader_limits)
                .map_err(|error| {
                CacheError::new("generation-reference-open-failed", error.message)
            })? {
                CacheLookup::Hit(reader) => {
                    if reader.completeness() != reference.completeness {
                        corrupt_code =
                            Some("generation-reference-completeness-mismatch".to_string());
                        continue;
                    }
                    return Ok(CacheLookup::Hit(LocatedGeneration { reference, reader }));
                }
                CacheLookup::Miss => {
                    stale_code = Some("generation-reference-target-missing".to_string())
                }
                CacheLookup::Stale { code } => stale_code = Some(code),
                CacheLookup::Corrupt { code } => corrupt_code = Some(code),
            }
        }
        if let Some(code) = corrupt_code {
            Ok(CacheLookup::Corrupt { code })
        } else if let Some(code) = stale_code {
            Ok(CacheLookup::Stale { code })
        } else {
            Ok(CacheLookup::Miss)
        }
    }

    pub fn publish_exact(
        &self,
        locator: &RepositoryLocator,
        compatibility: &GenerationCompatibility,
        identity: &GraphGenerationIdentity,
        completeness: Completeness,
        manifest_files: usize,
        manifest_bytes: u64,
    ) -> Result<PublishResult, CacheError> {
        Ok(self
            .publish_exact_tracked(
                locator,
                compatibility,
                identity,
                completeness,
                manifest_files,
                manifest_bytes,
            )?
            .result)
    }

    pub(crate) fn publish_exact_tracked(
        &self,
        locator: &RepositoryLocator,
        compatibility: &GenerationCompatibility,
        identity: &GraphGenerationIdentity,
        completeness: Completeness,
        manifest_files: usize,
        manifest_bytes: u64,
    ) -> Result<GenerationPublishOutcome, CacheError> {
        validate_lookup(locator, compatibility)?;
        identity.validate().map_err(|error| {
            CacheError::new(
                "generation-reference-identity-invalid",
                format!("invalid generation reference identity: {error}"),
            )
        })?;
        if identity.graph_schema_version != compatibility.graph_schema_version
            || identity.resolver_digest != compatibility.resolver_digest
            || identity.adapter_query_digest != compatibility.adapter_query_digest
            || identity.normalization_rules_digest != compatibility.normalization_rules_digest
        {
            return Err(CacheError::new(
                "generation-reference-identity-incompatible",
                "generation identity does not match locator compatibility",
            ));
        }
        let generation_key = identity.generation_key().map_err(|error| {
            CacheError::new("generation-reference-identity-invalid", error.to_string())
        })?;
        let exact = GenerationLookup::Exact {
            locator: locator.clone(),
        };
        let mut lookups = vec![exact];
        match locator.source {
            ReviewSource::Branch => {
                if let Some(tree) = &locator.base_tree {
                    lookups.push(GenerationLookup::BaseTree {
                        object_format: locator.object_format.clone(),
                        tree: tree.clone(),
                    });
                }
            }
            ReviewSource::Staged => {
                if let Some(index_manifest_digest) = &locator.index_manifest_digest {
                    lookups.push(GenerationLookup::IndexManifest {
                        object_format: locator.object_format.clone(),
                        index_manifest_digest: index_manifest_digest.clone(),
                    });
                }
            }
            ReviewSource::Unstaged => {}
        }
        let mut outcome = PublishResult::Reused;
        let mut published_paths = Vec::new();
        for lookup in lookups {
            let published_path = self
                .bucket(&lookup, compatibility)?
                .join(format!("{generation_key}.json"));
            let published = self.publish_lookup(
                lookup,
                locator,
                compatibility,
                identity,
                &generation_key,
                completeness,
                manifest_files,
                manifest_bytes,
            )?;
            if published == PublishResult::Published {
                outcome = PublishResult::Published;
                published_paths.push(published_path);
            }
        }
        Ok(GenerationPublishOutcome {
            result: outcome,
            published_paths,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_lookup(
        &self,
        lookup: GenerationLookup,
        locator: &RepositoryLocator,
        compatibility: &GenerationCompatibility,
        identity: &GraphGenerationIdentity,
        generation_key: &str,
        completeness: Completeness,
        manifest_files: usize,
        manifest_bytes: u64,
    ) -> Result<PublishResult, CacheError> {
        let reference = GenerationReference {
            lookup: lookup.clone(),
            locator: locator.clone(),
            compatibility: compatibility.clone(),
            identity: identity.clone(),
            generation_key: generation_key.to_string(),
            completeness,
            manifest_files,
            manifest_bytes,
        };
        let payload = serde_json::to_vec(&reference).map_err(|error| {
            CacheError::new(
                "generation-reference-encode-failed",
                format!("cannot encode generation reference: {error}"),
            )
        })?;
        let envelope = GenerationReferenceEnvelope {
            magic: REFERENCE_MAGIC.to_string(),
            schema_version: REFERENCE_SCHEMA_VERSION,
            payload_length: payload.len(),
            payload_sha256: sha256_hex(&payload),
            payload: reference,
        };
        let encoded = serde_json::to_vec(&envelope).map_err(|error| {
            CacheError::new(
                "generation-reference-encode-failed",
                format!("cannot encode generation reference envelope: {error}"),
            )
        })?;
        if encoded.len() > MAXIMUM_REFERENCE_BYTES {
            return Err(CacheError::new(
                "generation-reference-too-large",
                "encoded generation reference exceeds its byte limit",
            ));
        }

        self.layout.ensure_private_directories()?;
        let bucket = self.bucket(&lookup, compatibility)?;
        create_private_directory(&self.layout.graphs_dir.join("locators"))?;
        create_private_directory(&self.layout.graphs_dir.join("locators").join("v1"))?;
        let parent = bucket.parent().ok_or_else(|| {
            CacheError::new(
                "generation-reference-path-invalid",
                "generation reference bucket has no parent",
            )
        })?;
        create_private_directory(parent)?;
        create_private_directory(&bucket)?;
        let final_path = bucket.join(format!("{generation_key}.json"));
        match self.read_reference(&final_path, &lookup, compatibility)? {
            CacheLookup::Hit(existing) if existing == envelope.payload => {
                return Ok(PublishResult::Reused)
            }
            CacheLookup::Miss => {}
            CacheLookup::Hit(_) | CacheLookup::Stale { .. } | CacheLookup::Corrupt { .. } => {
                return Err(CacheError::new(
                    "generation-reference-conflict",
                    "an incompatible immutable generation reference already exists",
                ))
            }
        }

        let mut temporary = NamedTempFile::new_in(&bucket).map_err(|error| {
            CacheError::new(
                "generation-reference-temporary-create-failed",
                format!("cannot create generation reference staging file: {error}"),
            )
        })?;
        set_private_file_permissions(temporary.as_file())?;
        temporary.write_all(&encoded).map_err(|error| {
            CacheError::new(
                "generation-reference-write-failed",
                format!("cannot write generation reference: {error}"),
            )
        })?;
        temporary.as_file().sync_all().map_err(|error| {
            CacheError::new(
                "generation-reference-sync-failed",
                format!("cannot sync generation reference: {error}"),
            )
        })?;
        match temporary.persist_noclobber(&final_path) {
            Ok(_) => {
                sync_directory(&bucket)?;
                Ok(PublishResult::Published)
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                match self.read_reference(&final_path, &lookup, compatibility)? {
                    CacheLookup::Hit(existing) if existing == envelope.payload => {
                        Ok(PublishResult::Reused)
                    }
                    _ => Err(CacheError::new(
                        "generation-reference-conflict",
                        "concurrent writer published an incompatible generation reference",
                    )),
                }
            }
            Err(error) => Err(CacheError::new(
                "generation-reference-publish-failed",
                format!("cannot publish generation reference: {}", error.error),
            )),
        }
    }

    pub fn exact_lookup_digest(
        &self,
        locator: &RepositoryLocator,
        compatibility: &GenerationCompatibility,
    ) -> Result<String, CacheError> {
        validate_lookup(locator, compatibility)?;
        lookup_digest(
            &GenerationLookup::Exact {
                locator: locator.clone(),
            },
            compatibility,
        )
    }

    fn bucket(
        &self,
        lookup: &GenerationLookup,
        compatibility: &GenerationCompatibility,
    ) -> Result<PathBuf, CacheError> {
        let digest = lookup_digest(lookup, compatibility)?;
        Ok(self
            .layout
            .graphs_dir
            .join("locators")
            .join("v1")
            .join(&digest[..2])
            .join(digest))
    }

    fn read_reference(
        &self,
        path: &Path,
        lookup: &GenerationLookup,
        compatibility: &GenerationCompatibility,
    ) -> Result<CacheLookup<GenerationReference>, CacheError> {
        match self.decode_reference(path)? {
            CacheLookup::Hit(reference) => {
                if reference.lookup != *lookup || reference.compatibility != *compatibility {
                    Ok(CacheLookup::Stale {
                        code: "generation-reference-lookup-mismatch".to_string(),
                    })
                } else {
                    Ok(CacheLookup::Hit(reference))
                }
            }
            CacheLookup::Miss => Ok(CacheLookup::Miss),
            CacheLookup::Stale { code } => Ok(CacheLookup::Stale { code }),
            CacheLookup::Corrupt { code } => Ok(CacheLookup::Corrupt { code }),
        }
    }

    pub(crate) fn validate_reference_path(
        &self,
        path: &Path,
        reader_limits: ReaderLimits,
    ) -> Result<CacheLookup<()>, CacheError> {
        let reference = match self.decode_reference(path)? {
            CacheLookup::Hit(reference) => reference,
            CacheLookup::Miss => return Ok(CacheLookup::Miss),
            CacheLookup::Stale { code } => return Ok(CacheLookup::Stale { code }),
            CacheLookup::Corrupt { code } => return Ok(CacheLookup::Corrupt { code }),
        };
        let generation_path = self
            .layout
            .graphs_dir
            .join(format!("{}.sqlite", reference.generation_key));
        match RepositoryGraphReader::open_immutable(
            &generation_path,
            &reference.identity,
            reader_limits,
        )
        .map_err(|error| CacheError::new("generation-reference-open-failed", error.message))?
        {
            CacheLookup::Hit(reader) if reader.completeness() == reference.completeness => {
                Ok(CacheLookup::Hit(()))
            }
            CacheLookup::Hit(_) => Ok(corrupt("generation-reference-completeness-mismatch")),
            CacheLookup::Miss => Ok(CacheLookup::Stale {
                code: "generation-reference-target-missing".to_string(),
            }),
            CacheLookup::Stale { code } => Ok(CacheLookup::Stale { code }),
            CacheLookup::Corrupt { code } => Ok(CacheLookup::Corrupt { code }),
        }
    }

    fn decode_reference(
        &self,
        path: &Path,
    ) -> Result<CacheLookup<GenerationReference>, CacheError> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss)
            }
            Err(error) => {
                return Err(CacheError::new(
                    "generation-reference-metadata-unavailable",
                    format!("cannot inspect generation reference: {error}"),
                ))
            }
        };
        if !metadata.file_type().is_file() {
            return Ok(corrupt("generation-reference-not-regular"));
        }
        if metadata.len() > MAXIMUM_REFERENCE_BYTES as u64 {
            return Ok(corrupt("generation-reference-too-large"));
        }
        let mut file = open_regular_file_no_follow(path).map_err(|error| {
            CacheError::new(
                "generation-reference-open-failed",
                format!("cannot open generation reference: {error}"),
            )
        })?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(&mut file)
            .take(MAXIMUM_REFERENCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                CacheError::new(
                    "generation-reference-read-failed",
                    format!("cannot read generation reference: {error}"),
                )
            })?;
        if bytes.len() > MAXIMUM_REFERENCE_BYTES {
            return Ok(corrupt("generation-reference-too-large"));
        }
        let envelope: GenerationReferenceEnvelope = match serde_json::from_slice(&bytes) {
            Ok(envelope) => envelope,
            Err(_) => return Ok(corrupt("generation-reference-envelope-invalid")),
        };
        if envelope.magic != REFERENCE_MAGIC || envelope.schema_version != REFERENCE_SCHEMA_VERSION
        {
            return Ok(corrupt("generation-reference-envelope-incompatible"));
        }
        let payload = match serde_json::to_vec(&envelope.payload) {
            Ok(payload) => payload,
            Err(_) => return Ok(corrupt("generation-reference-payload-invalid")),
        };
        if payload.len() != envelope.payload_length
            || sha256_hex(&payload) != envelope.payload_sha256
        {
            return Ok(corrupt("generation-reference-checksum-mismatch"));
        }
        if envelope.payload.locator.validate().is_err()
            || envelope.payload.compatibility.validate().is_err()
            || !lookup_matches_locator(&envelope.payload.lookup, &envelope.payload.locator)
            || envelope.payload.identity.validate().is_err()
            || envelope.payload.identity.graph_schema_version
                != envelope.payload.compatibility.graph_schema_version
            || envelope.payload.identity.resolver_digest
                != envelope.payload.compatibility.resolver_digest
            || envelope.payload.identity.adapter_query_digest
                != envelope.payload.compatibility.adapter_query_digest
            || envelope.payload.identity.normalization_rules_digest
                != envelope.payload.compatibility.normalization_rules_digest
            || envelope.payload.identity.generation_key().as_deref()
                != Ok(envelope.payload.generation_key.as_str())
        {
            return Ok(corrupt("generation-reference-payload-invalid"));
        }
        let expected_name = format!("{}.json", envelope.payload.generation_key);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
            return Ok(corrupt("generation-reference-filename-mismatch"));
        }
        Ok(CacheLookup::Hit(envelope.payload))
    }
}

fn lookup_matches_locator(lookup: &GenerationLookup, locator: &RepositoryLocator) -> bool {
    match lookup {
        GenerationLookup::Exact { locator: exact } => exact == locator,
        GenerationLookup::BaseTree {
            object_format,
            tree,
        } => {
            locator.source == ReviewSource::Branch
                && locator.object_format == *object_format
                && locator.base_tree.as_deref() == Some(tree.as_str())
        }
        GenerationLookup::IndexManifest {
            object_format,
            index_manifest_digest,
        } => {
            locator.source == ReviewSource::Staged
                && locator.object_format == *object_format
                && locator.index_manifest_digest.as_deref() == Some(index_manifest_digest.as_str())
        }
    }
}

fn validate_directory_chain(root: &Path, directory: &Path) -> Result<CacheLookup<()>, CacheError> {
    let relative = directory.strip_prefix(root).map_err(|_| {
        CacheError::new(
            "generation-reference-path-invalid",
            "generation reference directory escapes the cache root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CacheLookup::Miss)
            }
            Err(error) => {
                return Err(CacheError::new(
                    "generation-reference-directory-unavailable",
                    format!("cannot inspect generation reference directory: {error}"),
                ))
            }
        };
        if is_symlink_or_reparse(&current, &metadata) || !metadata.file_type().is_dir() {
            return Ok(corrupt("generation-reference-directory-not-regular"));
        }
    }
    Ok(CacheLookup::Hit(()))
}

fn validate_lookup(
    locator: &RepositoryLocator,
    compatibility: &GenerationCompatibility,
) -> Result<(), CacheError> {
    locator.validate().map_err(|error| {
        CacheError::new(
            "generation-reference-locator-invalid",
            format!("invalid generation reference locator: {error}"),
        )
    })?;
    compatibility.validate()
}

fn lookup_digest(
    lookup: &GenerationLookup,
    compatibility: &GenerationCompatibility,
) -> Result<String, CacheError> {
    let encoded = serde_json::to_vec(&(lookup, compatibility)).map_err(|error| {
        CacheError::new(
            "generation-reference-key-encode-failed",
            format!("cannot encode generation reference key: {error}"),
        )
    })?;
    let mut digest = Sha256::new();
    hash_component(&mut digest, b"repository-generation-exact-locator/v1");
    hash_component(&mut digest, &encoded);
    Ok(format!("{:x}", digest.finalize()))
}

fn completeness_rank(value: Completeness) -> u8 {
    match value {
        Completeness::Complete => 2,
        Completeness::Partial => 1,
        Completeness::Unavailable => 0,
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_component(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn corrupt<T>(code: &str) -> CacheLookup<T> {
    CacheLookup::Corrupt {
        code: code.to_string(),
    }
}
