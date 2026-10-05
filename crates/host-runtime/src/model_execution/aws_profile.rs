//! Admission of one bounded AWS profile graph from captured configuration bytes.
//!
//! The pinned SDK parser reads only the captured contents: default files are off and
//! the environment and filesystem shims are empty. Admission resolves the selected
//! chain with the SDK's precedence, refuses graphs outside the supported commercial
//! source matrix and named forbidden options (the parser exposes no property
//! iteration), emits only allowlisted fields, and requires the emission to reparse to
//! the same graph.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};
use aws_runtime::env_config::section::{EnvConfigSections, Profile};
use aws_runtime::env_config::source;
use aws_types::os_shim_internal::{Env, Fs};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Upper bound on the captured config and credentials contents, each.
pub const MAX_CONFIG_FILE_BYTES: usize = 256 * 1024;
/// Upper bound on AssumeRole edges in one admitted graph.
pub const MAX_ROLE_EDGES: usize = 4;
/// Upper bound on profiles visited by one admitted graph. Every profile before the
/// root holds one role edge, so the edge bound enforces it.
pub const MAX_PROFILES: usize = MAX_ROLE_EDGES + 1;
/// Upper bound on the selected profile name and every visited section name.
pub const MAX_PROFILE_NAME_BYTES: usize = 256;
/// Upper bound on a region name.
pub const MAX_REGION_BYTES: usize = 64;
/// Role-session name used when a role edge does not name one.
pub const DEFAULT_ROLE_SESSION_NAME: &str = "eidnara-credentials";
/// The only supported SSO registration scope.
pub const SSO_ACCOUNT_ACCESS_SCOPE: &str = "sso:account:access";
/// The only supported AssumeRole duration; the pinned profile executor sends none,
/// so STS applies its 3600-second default.
pub const ROLE_DURATION_SECONDS: u32 = 3600;

/// Options selecting an unsupported source, destination, or trust root.
#[rustfmt::skip]
const FORBIDDEN_PROFILE_OPTIONS: &[&str] = &[
    "credential_process", "credential_source", "web_identity_token_file", "login_session",
    "mfa_serial", "aws_session_token", "sso_start_url", "sso_region", "endpoint_url",
    "use_fips_endpoint", "use_dualstack_endpoint", "sts_regional_endpoints", "services",
    "ca_bundle",
];

/// Options that alter the destination or trust of the SSO session's requests.
#[rustfmt::skip]
const FORBIDDEN_SESSION_OPTIONS: &[&str] = &[
    "endpoint_url", "use_fips_endpoint", "use_dualstack_endpoint", "services", "ca_bundle",
];

/// Why a captured configuration was refused. Variants carry only static option and
/// field names, never captured values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    /// A captured file exceeds [`MAX_CONFIG_FILE_BYTES`].
    FileTooLarge,
    /// A captured file is not UTF-8 or the pinned parser rejected it.
    Unparseable,
    /// The selected profile name or region is malformed.
    InvalidSelector,
    /// The selected profile, a referenced profile, or the named `sso-session` is absent.
    MissingSection,
    /// The source-profile chain revisits a profile.
    Cycle,
    /// The graph exceeds [`MAX_PROFILES`] profiles or [`MAX_ROLE_EDGES`] role edges.
    GraphTooLarge,
    /// A visited section sets an unsupported option.
    ForbiddenOption(&'static str),
    /// A visited profile holds fields of two credential sources.
    AmbiguousProfile,
    /// A role edge lacks `source_profile`, or a root lacks a required field.
    IncompleteSource,
    /// The chain ends in a profile with no supported credential source.
    NoCredentialSource,
    /// The static root's access key is session-scoped (`ASIA` prefix).
    TemporaryStaticRoot,
    /// A static root does not feed a role.
    StaticRootWithoutRole,
    /// A profile `region` differs from the selected region.
    RegionConflict,
    /// A field is malformed, spans lines, exceeds its bound, or holds an unsupported
    /// value such as a non-commercial region or a duration other than 3600.
    InvalidValue(&'static str),
    /// The emitted graph did not reparse to the admitted graph.
    RoundTripMismatch,
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AWS profile graph refused: {self:?}")
    }
}

impl std::error::Error for AdmissionError {}

/// Captured owner input for one admission.
#[derive(Clone, Copy)]
pub struct CapturedProfileInput<'a> {
    /// Selected profile name.
    pub profile: &'a str,
    /// Selected region; every visited profile `region` must equal it.
    pub region: &'a str,
    /// Captured config file contents.
    pub config: &'a [u8],
    /// Captured shared-credentials file contents.
    pub credentials: &'a [u8],
}

/// One AssumeRole edge. Every edge uses [`ROLE_DURATION_SECONDS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleEdge {
    pub profile: String,
    pub role_arn: String,
    pub source_profile: String,
    /// [`DEFAULT_ROLE_SESSION_NAME`] when the profile names none.
    pub session_name: String,
    pub external_id: Option<String>,
}

/// Root credential source of an admitted graph, with the profile that holds it. An SSO
/// root always registers with [`SSO_ACCOUNT_ACCESS_SCOPE`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootIdentity {
    /// Modern SSO root. The start URL is identity metadata, never a request
    /// destination; `sso_region` is the SSO and OIDC endpoint region.
    Sso {
        profile: String,
        session_name: String,
        start_url: String,
        sso_region: String,
        account_id: String,
        role_name: String,
    },
    /// Long-term static access key that feeds the last role edge, identified by the
    /// SHA-256 of its secret.
    Static {
        profile: String,
        access_key_id: String,
        secret_sha256: [u8; 32],
    },
}

/// Nonsecret identity of an admitted graph. Equal identities admit the same SDK
/// interpretation; formatting, comments, ordering, and unselected sections do not
/// enter it, and a changed static secret changes its digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphIdentity {
    pub profile: String,
    /// Selected region, the STS destination region.
    pub region: String,
    /// Role edges ordered from the selected profile toward the root.
    pub roles: Vec<RoleEdge>,
    pub root: RootIdentity,
}

/// A validated selected graph and its canonical emission.
pub struct AdmittedGraph {
    identity: GraphIdentity,
    static_secret: Option<Zeroizing<String>>,
}

impl fmt::Debug for AdmittedGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdmittedGraph")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl AdmittedGraph {
    pub fn identity(&self) -> &GraphIdentity {
        &self.identity
    }

    /// Canonical config text holding only this graph's allowlisted fields. Root fields
    /// close the last emitted profile section, which is the root profile. The text
    /// carries the static secret when the root is static; zeroization covers this
    /// graph's owned secret and emission, while the SDK parser's internal copies drop
    /// unwiped. The buffer is allocated at the emission's exact length, so it never
    /// reallocates and leaves no unwiped copy behind.
    pub fn emit_config(&self) -> Zeroizing<String> {
        let mut len = 0;
        self.emit_lines(|prefix, value, suffix| len += prefix.len() + value.len() + suffix.len());
        let mut out = Zeroizing::new(String::with_capacity(len));
        self.emit_lines(|prefix, value, suffix| {
            out.push_str(prefix);
            out.push_str(value);
            out.push_str(suffix);
        });
        out
    }

    /// Passes each emitted line to `line` as its prefix, value, and suffix.
    fn emit_lines(&self, mut line: impl FnMut(&str, &str, &str)) {
        let GraphIdentity {
            region,
            roles,
            root,
            ..
        } = &self.identity;
        for e in roles {
            line("[profile ", &e.profile, "]\n");
            line("region = ", region, "\n");
            line("role_arn = ", &e.role_arn, "\n");
            line("source_profile = ", &e.source_profile, "\n");
            line("role_session_name = ", &e.session_name, "\n");
            if let Some(external_id) = &e.external_id {
                line("external_id = ", external_id, "\n");
            }
        }
        let (RootIdentity::Sso { profile, .. } | RootIdentity::Static { profile, .. }) = root;
        if roles.last().is_none_or(|edge| edge.profile != *profile) {
            line("[profile ", profile, "]\n");
            line("region = ", region, "\n");
        }
        match root {
            RootIdentity::Sso {
                session_name: s,
                start_url,
                sso_region,
                account_id,
                role_name,
                ..
            } => {
                line("sso_session = ", s, "\n");
                line("sso_account_id = ", account_id, "\n");
                line("sso_role_name = ", role_name, "\n");
                line("[sso-session ", s, "]\n");
                line("sso_region = ", sso_region, "\n");
                line("sso_start_url = ", start_url, "\n");
                line("sso_registration_scopes = ", SSO_ACCOUNT_ACCESS_SCOPE, "\n");
            }
            RootIdentity::Static { access_key_id, .. } => {
                let secret = self.static_secret.as_deref().map_or("", String::as_str);
                line("aws_access_key_id = ", access_key_id, "\n");
                line("aws_secret_access_key = ", secret, "\n");
            }
        }
    }
}

/// Admits the selected graph of `input` and proves its canonical emission reparses to
/// the same graph.
pub fn admit(input: CapturedProfileInput<'_>) -> Result<AdmittedGraph, AdmissionError> {
    let too_large = |file: &[u8]| file.len() > MAX_CONFIG_FILE_BYTES;
    if too_large(input.config) || too_large(input.credentials) {
        return Err(AdmissionError::FileTooLarge);
    }
    let text = |file| std::str::from_utf8(file).map_err(|_| AdmissionError::Unparseable);
    let (config, credentials) = (text(input.config)?, text(input.credentials)?);
    let graph = admit_text(input.profile, input.region, config, credentials)?;
    let reparsed = admit_text(input.profile, input.region, &graph.emit_config(), "")?;
    if reparsed.identity != graph.identity {
        return Err(AdmissionError::RoundTripMismatch);
    }
    Ok(graph)
}

fn admit_text(
    profile: &str,
    region: &str,
    config: &str,
    credentials: &str,
) -> Result<AdmittedGraph, AdmissionError> {
    let selector = section_name(profile, "profile").and(validate_region(region, "region"));
    selector.map_err(|_| AdmissionError::InvalidSelector)?;
    let files = EnvConfigFiles::builder()
        .include_default_config_file(false)
        .include_default_credentials_file(false)
        .with_contents(EnvConfigFileKind::Config, config)
        .with_contents(EnvConfigFileKind::Credentials, credentials)
        .build();
    let (env, fs) = (Env::from_slice(&[]), Fs::from_slice(&[]));
    let mut loaded = poll_ready(source::load(&env, &fs, &files))
        .ok_or(AdmissionError::Unparseable)?
        .map_err(|_| AdmissionError::Unparseable)?;
    loaded.profile = Cow::Owned(profile.to_owned());
    let sections = EnvConfigSections::parse(loaded).map_err(|_| AdmissionError::Unparseable)?;
    resolve(&sections, profile, region)
}

/// Polls a future once. Loading in-memory contents never suspends, so a pending
/// result reports a parser contract change as a refusal.
fn poll_ready<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}

/// Resolves the selected chain with the SDK's order: a profile with `role_arn` is a
/// role edge toward its `source_profile`, and the first profile without one, or a
/// self-referencing edge, supplies the root. After the selected profile the SDK
/// prefers static keys over `role_arn`, so a source profile holding both is refused,
/// as is any profile holding two credential sources.
fn resolve(
    sections: &EnvConfigSections,
    selected: &str,
    region: &str,
) -> Result<AdmittedGraph, AdmissionError> {
    let mut visited: Vec<&str> = Vec::new();
    let mut roles = Vec::new();
    let mut name = selected;
    let (root, static_secret) = loop {
        if visited.contains(&name) {
            return Err(AdmissionError::Cycle);
        }
        let profile = sections
            .get_profile(name)
            .ok_or(AdmissionError::MissingSection)?;
        visited.push(name);
        check_profile(profile, region)?;
        let has = |keys: &[&str]| keys.iter().any(|key| profile.get(key).is_some());
        let has_static = has(&["aws_access_key_id", "aws_secret_access_key"]);
        let has_sso = has(&["sso_session", "sso_account_id", "sso_role_name"]);
        let Some(role_arn) = profile.get("role_arn") else {
            let role_options = [
                "source_profile",
                "role_session_name",
                "external_id",
                "duration_seconds",
            ];
            if has(&role_options) {
                return Err(AdmissionError::AmbiguousProfile);
            }
            break root_of(sections, profile, has_static, has_sso)?;
        };
        if roles.len() == MAX_ROLE_EDGES {
            return Err(AdmissionError::GraphTooLarge);
        }
        let source = profile
            .get("source_profile")
            .ok_or(AdmissionError::IncompleteSource)?;
        let source = section_name(source, "source_profile")?;
        roles.push(role_edge(profile, role_arn, source)?);
        if source == name {
            if visited.len() > 1 && has_static {
                return Err(AdmissionError::AmbiguousProfile);
            }
            break root_of(sections, profile, has_static, has_sso)?;
        }
        if has_static || has_sso {
            return Err(AdmissionError::AmbiguousProfile);
        }
        name = source;
    };
    if matches!(root, RootIdentity::Static { .. }) && roles.is_empty() {
        return Err(AdmissionError::StaticRootWithoutRole);
    }
    let (profile, region) = (selected.to_owned(), region.to_owned());
    let identity = GraphIdentity {
        profile,
        region,
        roles,
        root,
    };
    Ok(AdmittedGraph {
        identity,
        static_secret,
    })
}

fn forbid(present: impl Fn(&str) -> bool, options: &[&'static str]) -> Result<(), AdmissionError> {
    match options.iter().find(|option| present(option)) {
        Some(option) => Err(AdmissionError::ForbiddenOption(option)),
        None => Ok(()),
    }
}

fn check_profile(profile: &Profile, region: &str) -> Result<(), AdmissionError> {
    section_name(profile.name(), "profile")?;
    forbid(|key| profile.get(key).is_some(), FORBIDDEN_PROFILE_OPTIONS)?;
    match profile.get("region") {
        Some(own) if own != region => Err(AdmissionError::RegionConflict),
        _ => Ok(()),
    }
}

fn role_edge(profile: &Profile, role_arn: &str, source: &str) -> Result<RoleEdge, AdmissionError> {
    validate_role_arn(role_arn)?;
    let session_name = profile
        .get("role_session_name")
        .unwrap_or(DEFAULT_ROLE_SESSION_NAME);
    let session_name = charset(session_name, "role_session_name", 2, 64, b"+=,.@-")?;
    let external_id = profile.get("external_id");
    if let Some(external_id) = external_id {
        charset(external_id, "external_id", 2, 1224, b"+=,.@:/-")?;
    }
    if profile.get("duration_seconds").is_some_and(|d| d != "3600") {
        return Err(AdmissionError::InvalidValue("duration_seconds"));
    }
    Ok(RoleEdge {
        profile: profile.name().to_owned(),
        role_arn: role_arn.to_owned(),
        source_profile: source.to_owned(),
        session_name: session_name.to_owned(),
        external_id: external_id.map(str::to_owned),
    })
}

type Root = (RootIdentity, Option<Zeroizing<String>>);

fn root_of(
    sections: &EnvConfigSections,
    profile: &Profile,
    has_static: bool,
    has_sso: bool,
) -> Result<Root, AdmissionError> {
    match (has_sso, has_static) {
        (true, true) => Err(AdmissionError::AmbiguousProfile),
        (true, false) => Ok((sso_root(sections, profile)?, None)),
        (false, true) => static_root(profile),
        (false, false) => Err(AdmissionError::NoCredentialSource),
    }
}

fn sso_root(
    sections: &EnvConfigSections,
    profile: &Profile,
) -> Result<RootIdentity, AdmissionError> {
    let fields = (
        profile.get("sso_session"),
        profile.get("sso_account_id"),
        profile.get("sso_role_name"),
    );
    let (Some(session_name), Some(account_id), Some(role_name)) = fields else {
        return Err(AdmissionError::IncompleteSource);
    };
    let session_name = section_name(session_name, "sso_session")?;
    digits(account_id, "sso_account_id")?;
    charset(role_name, "sso_role_name", 1, 32, b"+=,.@-")?;
    let session = sections
        .sso_session(session_name)
        .ok_or(AdmissionError::MissingSection)?;
    forbid(|key| session.get(key).is_some(), FORBIDDEN_SESSION_OPTIONS)?;
    let (Some(sso_region), Some(start_url)) =
        (session.get("sso_region"), session.get("sso_start_url"))
    else {
        return Err(AdmissionError::IncompleteSource);
    };
    validate_region(sso_region, "sso_region")?;
    validate_start_url(start_url)?;
    let scope = session.get("sso_registration_scopes");
    if scope.is_some_and(|scope| scope != SSO_ACCOUNT_ACCESS_SCOPE) {
        return Err(AdmissionError::InvalidValue("sso_registration_scopes"));
    }
    Ok(RootIdentity::Sso {
        profile: profile.name().to_owned(),
        session_name: session_name.to_owned(),
        start_url: start_url.to_owned(),
        sso_region: sso_region.to_owned(),
        account_id: account_id.to_owned(),
        role_name: role_name.to_owned(),
    })
}

fn static_root(profile: &Profile) -> Result<Root, AdmissionError> {
    let keys = (
        profile.get("aws_access_key_id"),
        profile.get("aws_secret_access_key"),
    );
    let (Some(access_key_id), Some(secret)) = keys else {
        return Err(AdmissionError::IncompleteSource);
    };
    charset(access_key_id, "aws_access_key_id", 16, 128, b"")?;
    if access_key_id.starts_with("ASIA") {
        return Err(AdmissionError::TemporaryStaticRoot);
    }
    charset(secret, "aws_secret_access_key", 16, 128, b"/+=")?;
    let root = RootIdentity::Static {
        profile: profile.name().to_owned(),
        access_key_id: access_key_id.to_owned(),
        secret_sha256: Sha256::digest(secret.as_bytes()).into(),
    };
    Ok((root, Some(Zeroizing::new(secret.to_owned()))))
}

/// Accepts ASCII alphanumerics, `_`, and the listed punctuation within byte bounds,
/// which excludes whitespace and every line break.
fn charset<'a>(
    value: &'a str,
    field: &'static str,
    min: usize,
    max: usize,
    punctuation: &[u8],
) -> Result<&'a str, AdmissionError> {
    let allowed = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || punctuation.contains(&b);
    if (min..=max).contains(&value.len()) && value.bytes().all(allowed) {
        Ok(value)
    } else {
        Err(AdmissionError::InvalidValue(field))
    }
}

/// Section names follow the SDK identifier alphabet, which excludes whitespace, `[`,
/// and `]`, so an emitted name cannot open another section.
fn section_name<'a>(value: &'a str, field: &'static str) -> Result<&'a str, AdmissionError> {
    charset(value, field, 1, MAX_PROFILE_NAME_BYTES, b"-/.%@:+")
}

fn digits(value: &str, field: &'static str) -> Result<(), AdmissionError> {
    if value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(AdmissionError::InvalidValue(field))
    }
}

/// Commercial-partition region names: `<area>-<word>-<digits>` with an area from the
/// partition's region pattern and no GovCloud word.
fn validate_region(region: &str, field: &'static str) -> Result<(), AdmissionError> {
    const AREAS: &[&str] = &["us", "eu", "ap", "sa", "ca", "me", "af", "il", "mx"];
    let parts: Vec<&str> = region.split('-').collect();
    let valid = region.len() <= MAX_REGION_BYTES
        && matches!(parts.as_slice(), [area, word, number]
            if AREAS.contains(area)
                && !word.is_empty()
                && word.bytes().all(|b| b.is_ascii_lowercase())
                && *word != "gov"
                && (1..=3).contains(&number.len())
                && number.bytes().all(|b| b.is_ascii_digit()));
    if valid {
        Ok(())
    } else {
        Err(AdmissionError::InvalidValue(field))
    }
}

/// `arn:aws:iam::<account>:role/<path/>name` in the commercial partition.
fn validate_role_arn(arn: &str) -> Result<(), AdmissionError> {
    let invalid = AdmissionError::InvalidValue("role_arn");
    if arn.len() > 2048 {
        return Err(invalid);
    }
    let Some(rest) = arn.strip_prefix("arn:aws:iam::") else {
        return Err(invalid);
    };
    let (account, resource) = rest.split_once(':').ok_or(invalid)?;
    digits(account, "role_arn")?;
    let path_and_name = resource.strip_prefix("role/").ok_or(invalid)?;
    let name = path_and_name.rsplit('/').next().unwrap_or_default();
    charset(name, "role_arn", 1, 64, b"+=,.@-")?;
    charset(path_and_name, "role_arn", 1, 2048, b"+=,.@-/")?;
    Ok(())
}

/// HTTPS start URL with a multi-label DNS host, an optional nonzero port, and no
/// userinfo, query, or fragment.
fn validate_start_url(url: &str) -> Result<(), AdmissionError> {
    let invalid = Err(AdmissionError::InvalidValue("sso_start_url"));
    if url.len() > 2048
        || !url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"?#@\\".contains(&b))
    {
        return invalid;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return invalid;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let (host, port) = authority.split_once(':').unwrap_or((authority, "443"));
    let labels: Vec<&str> = host.split('.').collect();
    let label =
        |l: &&str| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let host_valid = labels.len() >= 2
        && labels.iter().all(label)
        && labels
            .last()
            .is_some_and(|l| !l.bytes().all(|b| b.is_ascii_digit()));
    match port.parse::<u16>() {
        Ok(number) if number != 0 && host_valid && port.bytes().all(|b| b.is_ascii_digit()) => {
            Ok(())
        }
        _ => invalid,
    }
}
