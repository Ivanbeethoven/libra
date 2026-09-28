//! Implements `verify-pack` for validating `.idx` files against their pack.

use std::path::{Path, PathBuf};

use clap::Parser;
use git_internal::hash::{HashKind, get_hash_kind, set_hash_kind};

use super::{
    verify_pack_decode::{decode_pack, validate_index_against_pack},
    verify_pack_index::{idx_v2_matches_hash_kind, parse_index},
    verify_pack_render::{
        VerifyPackRenderMode, build_object_outputs, build_stats, render_verify_pack_batch_output,
        render_verify_pack_output,
    },
    verify_pack_support::{
        bytes_to_hex, invalid_index, path_string, read_file, verification_failed,
    },
    verify_pack_types::VerifyPackOutput,
};
use crate::utils::{
    error::{CliError, CliResult, StableErrorCode},
    output::OutputConfig,
    util,
};

const VERIFY_PACK_EXAMPLES: &str = "\
EXAMPLES:
    libra verify-pack objects/pack/pack-abc123.idx                   Verify an index against its sibling .pack
    libra verify-pack pack-a.idx pack-b.idx                          Verify multiple indexes
    libra verify-pack --pack pack.pack pack.idx                      Verify with an explicit pack path
    libra verify-pack -v pack-abc123.idx                             Print every indexed object hash and offset
    libra verify-pack -s pack-abc123.idx                             Print only pack statistics
    libra verify-pack --hash-kind sha256 pack.idx                    Outside a repo, declare the index hash kind
    libra verify-pack pack-abc123.idx --json                         Structured JSON output for agents";

#[derive(Parser, Debug)]
#[command(after_help = VERIFY_PACK_EXAMPLES)]
pub struct VerifyPackArgs {
    #[arg(
        value_name = "IDX_FILE",
        num_args = 1..,
        help = "Pack index file(s) to verify"
    )]
    pub idx_files: Vec<PathBuf>,

    /// Pack file to verify against. Defaults to IDX_FILE with `.pack` extension.
    #[arg(long, value_name = "PACK_FILE")]
    pub pack: Option<PathBuf>,

    /// Print every indexed object hash and offset
    #[arg(short, long, conflicts_with = "stat_only")]
    pub verbose: bool,

    /// Show pack statistics only
    #[arg(short = 's', long = "stat-only", conflicts_with = "verbose")]
    pub stat_only: bool,

    /// Object-format / hash kind used to parse the index (`sha1`, `sha256`, or
    /// `blake3`). Required outside a repository; inside a repository the
    /// stored `core.objectformat` is used and this flag must match when set.
    #[arg(long = "hash-kind", value_name = "KIND")]
    pub hash_kind: Option<String>,
}

pub async fn execute(args: VerifyPackArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// # Side Effects
///
/// This command is read-only. It reads the requested `.idx` file and matching
/// `.pack` file, decodes the pack, and reports whether the index is consistent.
///
/// # Errors
///
/// Returns structured CLI errors for unreadable files and repository-corruption
/// errors for malformed indexes, malformed packs, or index/pack mismatches.
pub async fn execute_safe(args: VerifyPackArgs, output: &OutputConfig) -> CliResult<()> {
    if args.idx_files.is_empty() {
        return Err(
            CliError::fatal("verify-pack requires at least one index file")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }
    if args.pack.is_some() && args.idx_files.len() > 1 {
        return Err(
            CliError::fatal("cannot use --pack with multiple index files")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }

    let hash_kind = resolve_verify_pack_hash_kind(args.hash_kind.as_deref())?;
    set_hash_kind(hash_kind);

    let mode = render_mode(&args);
    let results = args
        .idx_files
        .iter()
        .map(|idx_file| verify_pack(&args, idx_file, hash_kind))
        .collect::<CliResult<Vec<_>>>()?;

    if let [result] = results.as_slice() {
        render_verify_pack_output(result, mode, output)
    } else {
        render_verify_pack_batch_output(&results, mode, output)
    }
}

fn resolve_verify_pack_hash_kind(explicit: Option<&str>) -> CliResult<HashKind> {
    let in_repo = util::try_get_storage_path(None).is_ok();
    let parsed_explicit = explicit
        .map(|value| {
            crate::internal::object_format::parse_config_value(value).map_err(|_| {
                CliError::fatal(format!(
                    "unsupported --hash-kind '{value}'; expected sha1, sha256, or blake3"
                ))
                .with_stable_code(StableErrorCode::CliInvalidArguments)
            })
        })
        .transpose()?;

    match (in_repo, parsed_explicit) {
        (true, Some(kind)) => {
            let repo_kind = get_hash_kind();
            if kind != repo_kind {
                return Err(CliError::fatal(format!(
                    "--hash-kind {} does not match repository object format {}",
                    kind.as_str(),
                    repo_kind.as_str()
                ))
                .with_stable_code(StableErrorCode::CliInvalidArguments));
            }
            Ok(repo_kind)
        }
        (true, None) => Ok(get_hash_kind()),
        (false, Some(kind)) => Ok(kind),
        (false, None) => Err(CliError::fatal(
            "verify-pack outside a repository requires --hash-kind <sha1|sha256|blake3>; \
             run inside a Libra repository to use core.objectformat"
                .to_string(),
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments)),
    }
}

fn verify_pack(
    args: &VerifyPackArgs,
    idx_file: &Path,
    hash_kind: HashKind,
) -> CliResult<VerifyPackOutput> {
    let pack_file = args
        .pack
        .clone()
        .unwrap_or_else(|| idx_file.with_extension("pack"));

    let idx_bytes = read_file(idx_file, "pack index")?;
    // Fail closed when a v2 index's tables/trailer do not match the resolved kind
    // (sha256 vs blake3 share OID width, so guessing is forbidden).
    idx_v2_matches_hash_kind(&idx_bytes, hash_kind)
        .map_err(|detail| invalid_index(idx_file, detail))?;
    let parsed = parse_index(&idx_bytes).map_err(|detail| invalid_index(idx_file, detail))?;
    let decoded = decode_pack(&pack_file)?;
    validate_index_against_pack(&parsed, &decoded)
        .map_err(|detail| verification_failed(idx_file, &pack_file, detail))?;

    let objects = if args.verbose {
        build_object_outputs(&parsed, &decoded)?
    } else {
        Vec::new()
    };
    let stats = args.stat_only.then(|| build_stats(&decoded));

    Ok(VerifyPackOutput {
        idx_file: path_string(idx_file),
        pack_file: path_string(&pack_file),
        index_version: parsed.version,
        object_count: parsed.entries.len(),
        pack_hash: parsed.pack_hash.to_string(),
        index_hash: bytes_to_hex(&parsed.index_hash),
        verified: true,
        stats,
        objects,
    })
}

const fn render_mode(args: &VerifyPackArgs) -> VerifyPackRenderMode {
    if args.stat_only {
        VerifyPackRenderMode::StatOnly
    } else if args.verbose {
        VerifyPackRenderMode::Verbose
    } else {
        VerifyPackRenderMode::Summary
    }
}
