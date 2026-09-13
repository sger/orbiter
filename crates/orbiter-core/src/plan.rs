//! Re-signing plan: what would change if this IPA were signed by another team.
//!
//! Pure local computation over an inspection report. It signs nothing, contacts no Apple service,
//! and mutates no file. Every identifier rewrite and every removed capability is stated with its
//! runtime consequence so a person can accept or reject the plan before anything is produced.
//!
//! Capability support is *proposed*, not asserted: Apple's published table does not cleanly
//! separate Personal Team support, so each decision carries whether it still needs confirmation
//! from the portal. The portal's actual answer must override this plan when that work exists.
use crate::{Bundle, Report};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Apple limits a Personal Team to 10 App IDs per seven days.
const PERSONAL_APP_ID_BUDGET: usize = 10;
/// Bundle identifiers stay well inside Apple's accepted length after a rewrite.
const MAX_IDENTIFIER: usize = 155;

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TeamKind {
    /// A free Apple account's Personal Team: seven-day profiles, restricted capabilities.
    Personal,
    /// A paid membership.
    Paid,
}

/// What to do with a Watch app inside the IPA. Apple's provisioning for watchOS under a free
/// personal team is unverified here, and a Watch bundle consumes App IDs from a small weekly
/// budget, so the choice is made by a person before anything is registered — never silently.
#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum WatchChoice {
    /// No choice made yet. Blocks the plan while the IPA contains a Watch app.
    #[default]
    Undecided,
    /// Drop the Watch app and everything nested inside it from the re-signed build.
    Remove,
    /// Keep the Watch app and attempt to provision and sign it with the rest.
    Sign,
}

impl WatchChoice {
    /// Parse the interface's value. An unrecognised value is not a decision.
    pub fn parse(value: &str) -> Self {
        match value {
            "remove" => Self::Remove,
            "sign" => Self::Sign,
            _ => Self::Undecided,
        }
    }
    /// The inverse of `parse`, so a stored choice can be handed back to the interface unchanged.
    pub fn label(self) -> &'static str {
        match self {
            Self::Remove => "remove",
            Self::Sign => "sign",
            Self::Undecided => "undecided",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Target {
    pub team_id: String,
    pub kind: TeamKind,
    pub watch: WatchChoice,
    /// File names of libraries to inject into the main app and load at launch. Empty for a plain
    /// re-sign.
    pub injected_dylibs: Vec<String>,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Kept unchanged.
    Keep,
    /// Kept, with a value rewritten for the new team.
    Rewrite,
    /// Removed, because the target team cannot carry it.
    Remove,
}

#[derive(Clone, Serialize, Debug)]
pub struct Capability {
    pub key: String,
    pub action: Action,
    pub value: Option<String>,
    /// Why the action was chosen. Static text; never a server message.
    pub reason: &'static str,
    /// What the tester loses at runtime. Empty when nothing is lost.
    pub consequence: &'static str,
    /// True when only the portal can confirm this decision.
    pub needs_portal_confirmation: bool,
}

#[derive(Clone, Serialize, Debug)]
pub struct BundlePlan {
    pub path: String,
    pub kind: String,
    pub name: String,
    pub identifier: String,
    pub new_identifier: String,
    /// Bundles that consume one of the team's App IDs. Frameworks do not.
    pub consumes_app_id: bool,
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Serialize, Debug)]
pub struct Plan {
    pub team_id: String,
    pub team_kind: TeamKind,
    pub main_identifier: String,
    pub new_main_identifier: String,
    pub bundles: Vec<BundlePlan>,
    /// Conditions that stop re-signing until they are resolved.
    pub blockers: Vec<String>,
    /// Runtime consequences of the plan as a whole, for acknowledgement.
    pub consequences: Vec<String>,
    pub app_ids_required: usize,
    /// Libraries injected into the main app, by file name. Empty for a plain re-sign.
    pub injected_dylibs: Vec<String>,
}

/// Deterministic per-team suffix: the same team always produces the same identifiers, so a weekly
/// re-sign replaces the tester's app instead of installing a second copy beside it.
fn suffix(team_id: &str) -> String {
    let digest = Sha256::digest(team_id.as_bytes());
    digest[..4].iter().fold(String::new(), |mut out, byte| {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Rewrite one bundle identifier for the target team, preserving its relationship to the main app.
///
/// A nested bundle keeps the part of its identifier that sits under the main app's and is rebuilt
/// beneath the rewritten one, so a Watch app or an extension still reads as belonging to its
/// parent. An identifier unrelated to the main app is rewritten on its own.
///
/// Deterministic, because the same team must always produce the same identifiers: a weekly re-sign
/// has to replace the tester's app rather than install a second copy beside it.
fn rewrite_identifier(identifier: &str, main: &str, new_main: &str) -> String {
    let rewritten = match identifier.strip_prefix(main) {
        // Nested bundles are named under the main identifier; keep that relationship intact.
        Some(rest) => format!("{new_main}{rest}"),
        None => format!(
            "{identifier}.{}",
            new_main.rsplit('.').next().unwrap_or("orbiter")
        ),
    };
    if rewritten.len() > MAX_IDENTIFIER {
        rewritten[..MAX_IDENTIFIER]
            .trim_end_matches('.')
            .to_string()
    } else {
        rewritten
    }
}

/// Third-party SDKs in the build whose service checks the bundle identifier server-side.
///
/// Named only when their framework is actually present. This is not a complete list of what may
/// pin an identifier — a company's own backend commonly does — so the consequence says so too.
fn pinning_services(report: &Report) -> Vec<&'static str> {
    let mut named: Vec<&'static str> = report
        .bundles
        .iter()
        .filter_map(|bundle| match bundle.identifier.as_str() {
            id if id.starts_with("com.facebook.sdk") => Some("the Facebook SDK"),
            id if id.starts_with("com.google.GoogleSignIn") => Some("Google Sign-In"),
            _ => None,
        })
        .collect();
    named.sort();
    named.dedup();
    named
}

/// Whether a bundle of this kind needs an App ID reserved on the team.
///
/// Frameworks do not: they are signed but never provisioned. The distinction decides how much of a
/// free team's ten-per-seven-days budget a plan would spend, which is why it is refused up front
/// rather than discovered halfway through.
fn consumes_app_id(kind: &str) -> bool {
    // Frameworks are signed with the app's identity but hold no App ID of their own.
    kind != "Framework"
}

/// Decide one entitlement key. `personal` narrows what the target team can carry.
fn decide(key: &str, target: &Target, new_identifier: &str) -> Capability {
    let personal = target.kind == TeamKind::Personal;
    let team = &target.team_id;
    let (action, value, reason, consequence, confirm) = match key {
        "application-identifier" | "com.apple.application-identifier" => (
            Action::Rewrite,
            Some(format!("{team}.{new_identifier}")),
            "The application identifier must name the signing team and the new bundle identifier.",
            "",
            false,
        ),
        "com.apple.developer.team-identifier" => (
            Action::Rewrite,
            Some(team.clone()),
            "The team identifier must name the signing team.",
            "",
            false,
        ),
        "get-task-allow" => (
            Action::Rewrite,
            Some("true".into()),
            "Development signing requires the debug entitlement.",
            "",
            false,
        ),
        "keychain-access-groups" => (
            Action::Rewrite,
            None,
            "Keychain groups are prefixed with the signing team.",
            "Keychain items saved under the company team are not readable by the re-signed app.",
            false,
        ),
        "com.apple.security.application-groups" if personal => (
            Action::Remove,
            None,
            "A Personal Team cannot create app groups.",
            "The app and its extensions can no longer share group storage; features relying on shared data stop working.",
            true,
        ),
        "com.apple.security.application-groups" => (
            Action::Rewrite,
            None,
            "App groups are prefixed with the signing team.",
            "Data already stored in the company team's group is not carried over.",
            true,
        ),
        "aps-environment" | "com.apple.developer.aps-environment" if personal => (
            Action::Remove,
            None,
            "A Personal Team cannot create a push capability.",
            "Push notifications stop working in the re-signed app.",
            true,
        ),
        "com.apple.developer.associated-domains" if personal => (
            Action::Remove,
            None,
            "A Personal Team cannot create associated domains.",
            "Universal links open in the browser instead of the app, and web credential autofill stops.",
            true,
        ),
        "com.apple.developer.in-app-payments" if personal => (
            Action::Remove,
            None,
            "A Personal Team cannot create Apple Pay merchant identifiers.",
            "Apple Pay is unavailable in the re-signed app.",
            true,
        ),
        other if other.starts_with("com.apple.developer.") && personal => (
            Action::Remove,
            None,
            "This capability is not established as available to a Personal Team.",
            "The feature behind this capability may stop working; the portal decides whether it can be kept.",
            true,
        ),
        _ => (
            Action::Keep,
            None,
            "Not a team-scoped capability.",
            "",
            false,
        ),
    };
    Capability {
        key: key.to_string(),
        action,
        value,
        reason,
        consequence,
        needs_portal_confirmation: confirm,
    }
}

/// Every entitlement key a bundle carries, from its executable and its embedded profile together.
///
/// Both sources are merged because they answer different halves of the question: the profile says
/// what was authorised, the executable says what was actually claimed, and a capability appearing
/// in either has to be reckoned with. Sorted and deduplicated, so two plans for the same build
/// list capabilities in the same order.
fn entitlement_keys(bundle: &Bundle) -> Vec<String> {
    let mut keys: Vec<String> = bundle
        .slices
        .iter()
        .flat_map(|slice| slice.entitlements.keys().cloned())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Build the plan. Nothing is signed, written, or sent; the report is read only.
pub fn build(report: &Report, target: &Target) -> Plan {
    let main = report
        .bundles
        .iter()
        .find(|bundle| bundle.path == report.main_path)
        .or_else(|| report.bundles.first());
    let main_identifier = main.map(|b| b.identifier.clone()).unwrap_or_default();
    let new_main_identifier = if main_identifier.is_empty() {
        String::new()
    } else {
        format!("{main_identifier}.{}", suffix(&target.team_id))
    };

    let mut blockers = Vec::new();
    let mut consequences = Vec::new();
    if main_identifier.is_empty() {
        blockers.push(
            "The IPA has no readable main bundle identifier, so no identifier can be rewritten."
                .into(),
        );
    }
    if target.team_id.is_empty() || !target.team_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        blockers.push("Select a signing team before a plan can be produced.".into());
    }

    let watch_roots: Vec<&str> = report
        .bundles
        .iter()
        .filter(|bundle| bundle.kind == "Watch app")
        .map(|bundle| bundle.path.as_str())
        .collect();
    // A Watch app carries its own extensions and frameworks; removing it removes all of them.
    let inside_watch = |path: &str| {
        watch_roots
            .iter()
            .any(|root| path == *root || path.starts_with(&format!("{root}/")))
    };
    if !watch_roots.is_empty() {
        match target.watch {
            WatchChoice::Undecided => blockers.push(
                "This IPA contains a Watch app. Choose whether to remove it or attempt to sign it before any identifier is registered."
                    .into(),
            ),
            WatchChoice::Remove => consequences.push(
                "The Watch app is removed from the re-signed build, so its watchOS features are unavailable on a paired Watch."
                    .into(),
            ),
            WatchChoice::Sign => consequences.push(
                "The Watch app is signed with the rest of the build. Watch provisioning under this team is unverified, so it may fail to install or to run on a paired Watch."
                    .into(),
            ),
        }
    }

    let mut bundles = Vec::new();
    for bundle in &report.bundles {
        if target.watch == WatchChoice::Remove && inside_watch(&bundle.path) {
            continue;
        }
        if bundle.slices.iter().any(|slice| slice.encrypted) {
            blockers.push(format!(
                "{} has an encrypted executable, which cannot be re-signed.",
                bundle.name
            ));
        }
        let capabilities: Vec<Capability> = entitlement_keys(bundle)
            .iter()
            .map(|key| {
                decide(
                    key,
                    target,
                    &rewrite_identifier(&bundle.identifier, &main_identifier, &new_main_identifier),
                )
            })
            .collect();
        for capability in &capabilities {
            if !capability.consequence.is_empty()
                && !consequences.iter().any(|c| c == capability.consequence)
            {
                consequences.push(capability.consequence.to_string());
            }
        }
        bundles.push(BundlePlan {
            path: bundle.path.clone(),
            kind: bundle.kind.clone(),
            name: bundle.name.clone(),
            identifier: bundle.identifier.clone(),
            new_identifier: rewrite_identifier(
                &bundle.identifier,
                &main_identifier,
                &new_main_identifier,
            ),
            consumes_app_id: consumes_app_id(&bundle.kind),
            capabilities,
        });
    }

    let app_ids_required = bundles.iter().filter(|b| b.consumes_app_id).count();
    if target.kind == TeamKind::Personal && app_ids_required > PERSONAL_APP_ID_BUDGET {
        blockers.push(format!(
            "This IPA needs {app_ids_required} App IDs, above the {PERSONAL_APP_ID_BUDGET} a Personal Team can register in seven days."
        ));
    }
    // Nothing in the build changes here, and no entitlement is involved: the identifier itself is
    // the credential these services check, and it had to change for another team to sign at all.
    if !new_main_identifier.is_empty() {
        let named = pinning_services(report);
        consequences.push(format!(
            "The build installs as {new_main_identifier}, so any service that recognises the app by its bundle identifier will not recognise this one{}. Sign-in through those providers fails until the new identifier is registered with them, and a backend that pins the identifier refuses it too. Each team produces a different identifier, so each tester needs registering separately.",
            if named.is_empty() {
                String::new()
            } else {
                format!(" — this build embeds {}", named.join(" and "))
            }
        ));
    }
    if target.kind == TeamKind::Personal {
        consequences.push(
            "A Personal Team profile expires after seven days, so the app must be re-signed and reinstalled every week."
                .into(),
        );
    }
    if !target.injected_dylibs.is_empty() {
        consequences.push(format!(
            "{} added librar{} injected into the app and loaded at launch: {}. The executable no longer matches its author's build, the injected code runs with the app's entitlements, and the result is not App-Store installable.",
            target.injected_dylibs.len(),
            if target.injected_dylibs.len() == 1 { "y is" } else { "ies are" },
            target.injected_dylibs.join(", "),
        ));
    }

    Plan {
        team_id: target.team_id.clone(),
        team_kind: target.kind,
        main_identifier,
        new_main_identifier,
        bundles,
        blockers,
        consequences,
        app_ids_required,
        injected_dylibs: target.injected_dylibs.clone(),
    }
}

#[cfg(test)]
/// Checks what re-signing under another team would change, and what it refuses to decide alone.
mod tests {
    use super::*;
    use crate::macho::Slice;
    use std::collections::BTreeMap;

    /// One architecture slice carrying the given entitlement keys and values.
    fn slice(keys: &[(&str, &str)]) -> Slice {
        Slice {
            architecture: "arm64".into(),
            encrypted: false,
            entitlements: keys
                .iter()
                .map(|(k, v)| {
                    (
                        (*k).to_string(),
                        serde_json::Value::String((*v).to_string()),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
            xml_entitlements_present: true,
            der_entitlements_present: false,
        }
    }
    /// One bundle of the given kind, identifier and entitlements.
    fn bundle(path: &str, kind: &str, identifier: &str, slices: Vec<Slice>) -> Bundle {
        Bundle {
            path: path.into(),
            kind: kind.into(),
            name: identifier.into(),
            identifier: identifier.into(),
            version: None,
            build: None,
            minimum_os: None,
            supported_platforms: vec![],
            device_families: vec![],
            slices,
            profile: None,
            issues: vec![],
        }
    }
    /// A report whose main app is the first bundle given.
    fn report(bundles: Vec<Bundle>) -> Report {
        Report {
            size_bytes: 1,
            main_path: "Payload/App.app".into(),
            bundles,
            findings: vec![],
            icon_data_url: None,
        }
    }
    /// A free personal team as the signing target, with the Watch app kept.
    fn personal() -> Target {
        Target {
            team_id: "ABCDE12345".into(),
            kind: TeamKind::Personal,
            watch: WatchChoice::Sign,
            injected_dylibs: Vec::new(),
        }
    }

    #[test]
    /// Every bundle is rewritten under the target team, and a nested bundle still reads as
    /// belonging to its parent rather than becoming an unrelated identifier.
    fn identifiers_are_rewritten_and_keep_the_nesting_relationship() {
        let plan = build(
            &report(vec![
                bundle("Payload/App.app", "Main app", "com.company.app", vec![]),
                bundle(
                    "Payload/App.app/PlugIns/Share.appex",
                    "Extension",
                    "com.company.app.share",
                    vec![],
                ),
                bundle(
                    "Payload/App.app/Frameworks/Core.framework",
                    "Framework",
                    "com.company.core",
                    vec![],
                ),
            ]),
            &personal(),
        );
        let new_main = plan.new_main_identifier.clone();
        assert!(new_main.starts_with("com.company.app."));
        // The extension stays under the main app's new identifier.
        assert_eq!(plan.bundles[1].new_identifier, format!("{new_main}.share"));
        // Frameworks are signed but consume no App ID.
        assert!(!plan.bundles[2].consumes_app_id);
        assert_eq!(plan.app_ids_required, 2);
        assert!(plan.blockers.is_empty());
    }

    #[test]
    /// The same team always yields the same identifiers, so a weekly re-sign replaces the
    /// tester's app instead of installing a second copy beside it.
    fn the_same_team_always_produces_the_same_identifiers() {
        // A weekly re-sign must replace the tester's app, not install a second copy beside it.
        let ipa = || {
            report(vec![bundle(
                "Payload/App.app",
                "Main app",
                "com.company.app",
                vec![],
            )])
        };
        let first = build(&ipa(), &personal());
        let second = build(&ipa(), &personal());
        assert_eq!(first.new_main_identifier, second.new_main_identifier);
        let other = build(
            &ipa(),
            &Target {
                team_id: "ZZZZZ99999".into(),
                kind: TeamKind::Personal,
                watch: WatchChoice::Sign,
                injected_dylibs: Vec::new(),
            },
        );
        assert_ne!(first.new_main_identifier, other.new_main_identifier);
    }

    #[test]
    /// A free team cannot carry push, universal links, Apple Pay or app groups, so each is
    /// removed *and* stated as a consequence — losing one silently is what makes a build look
    /// broken for no reason.
    fn personal_team_capabilities_are_removed_with_their_consequence() {
        let plan = build(
            &report(vec![bundle(
                "Payload/App.app",
                "Main app",
                "com.company.app",
                vec![slice(&[
                    ("aps-environment", "production"),
                    ("com.apple.developer.associated-domains", "applinks:x"),
                    ("com.apple.developer.in-app-payments", "merchant.x"),
                    ("com.apple.security.application-groups", "group.x"),
                    ("keychain-access-groups", "TEAM.x"),
                    ("application-identifier", "TEAM.com.company.app"),
                    ("com.apple.developer.team-identifier", "TEAM"),
                    ("get-task-allow", "false"),
                ])],
            )]),
            &personal(),
        );
        let by = |key: &str| {
            plan.bundles[0]
                .capabilities
                .iter()
                .find(|c| c.key == key)
                .cloned()
                .expect("capability present")
        };
        for key in [
            "aps-environment",
            "com.apple.developer.associated-domains",
            "com.apple.developer.in-app-payments",
            "com.apple.security.application-groups",
        ] {
            let capability = by(key);
            assert_eq!(capability.action, Action::Remove, "{key}");
            assert!(!capability.consequence.is_empty(), "{key}");
            assert!(capability.needs_portal_confirmation, "{key}");
        }
        assert_eq!(by("keychain-access-groups").action, Action::Rewrite);
        assert_eq!(
            by("application-identifier").value.as_deref(),
            Some(format!("ABCDE12345.{}", plan.new_main_identifier).as_str())
        );
        assert_eq!(by("get-task-allow").value.as_deref(), Some("true"));
        assert_eq!(
            by("com.apple.developer.team-identifier").value.as_deref(),
            Some("ABCDE12345")
        );
        // Every removal's consequence reaches the acknowledgement list, plus weekly expiry.
        assert!(
            plan.consequences
                .iter()
                .any(|c| c.contains("Push notifications"))
        );
        assert!(plan.consequences.iter().any(|c| c.contains("seven days")));
    }

    #[test]
    /// A paid team keeps the capabilities a free one loses: the removals are a property of the
    /// target team, not of re-signing.
    fn a_paid_team_keeps_capabilities_a_personal_team_cannot_carry() {
        let ipa = || {
            report(vec![bundle(
                "Payload/App.app",
                "Main app",
                "com.company.app",
                vec![slice(&[("aps-environment", "production")])],
            )])
        };
        let paid = build(
            &ipa(),
            &Target {
                team_id: "PAID123456".into(),
                kind: TeamKind::Paid,
                watch: WatchChoice::Sign,
                injected_dylibs: Vec::new(),
            },
        );
        assert_eq!(paid.bundles[0].capabilities[0].action, Action::Keep);
        assert!(!paid.consequences.iter().any(|c| c.contains("seven days")));
    }

    #[test]
    /// An encrypted executable blocks the plan, and a Watch app blocks it until someone decides
    /// what happens to it.
    fn unsignable_inputs_block_and_watch_apps_require_an_explicit_choice() {
        let mut encrypted = slice(&[]);
        encrypted.encrypted = true;
        let plan = build(
            &report(vec![
                bundle(
                    "Payload/App.app",
                    "Main app",
                    "com.company.app",
                    vec![encrypted],
                ),
                bundle(
                    "Payload/App.app/Watch/Watch.app",
                    "Watch app",
                    "com.company.app.watch",
                    vec![],
                ),
            ]),
            &personal(),
        );
        assert!(plan.blockers.iter().any(|b| b.contains("encrypted")));
        // Signing a Watch app is a choice, and its uncertainty is stated rather than hidden.
        assert!(plan.consequences.iter().any(|c| c.contains("unverified")));
    }

    #[test]
    /// The rewritten identifier is stated as a consequence, because a backend or a social SDK
    /// that recognises the app by its bundle identifier will not recognise the signed build.
    fn the_rewritten_identifier_is_stated_as_something_services_will_not_recognise() {
        let plan = build(
            &report(vec![
                bundle("Payload/App.app", "Main app", "com.company.app", vec![]),
                bundle(
                    "Payload/App.app/Frameworks/FBSDKCoreKit.framework",
                    "Framework",
                    "com.facebook.sdk.FBSDKCoreKit",
                    vec![],
                ),
            ]),
            &personal(),
        );
        let stated = plan
            .consequences
            .iter()
            .find(|c| c.contains("bundle identifier"))
            .expect("the identifier change is a consequence in its own right");
        // The new identifier, the named SDK found in the build, and the per-tester cost.
        assert!(stated.contains(&plan.new_main_identifier));
        assert!(stated.contains("Facebook"));
        assert!(stated.contains("each tester"));
    }

    #[test]
    /// A Watch app is never removed silently and never kept silently: while the choice is open
    /// the plan is blocked, and either decision becomes a stated consequence.
    fn a_watch_app_blocks_the_plan_until_a_person_chooses_what_happens_to_it() {
        let bundles = || {
            vec![
                bundle("Payload/App.app", "Main app", "com.company.app", vec![]),
                bundle(
                    "Payload/App.app/Watch/Watch.app",
                    "Watch app",
                    "com.company.app.watch",
                    vec![],
                ),
                bundle(
                    "Payload/App.app/Watch/Watch.app/PlugIns/W.appex",
                    "Extension",
                    "com.company.app.watch.ext",
                    vec![],
                ),
            ]
        };
        let undecided = build(
            &report(bundles()),
            &Target {
                team_id: "ABCDE12345".into(),
                kind: TeamKind::Personal,
                watch: WatchChoice::Undecided,
                injected_dylibs: Vec::new(),
            },
        );
        // Nothing is registered while the choice is open: App IDs are spent from a weekly budget.
        assert!(undecided.blockers.iter().any(|b| b.contains("Watch app")));

        let removed = build(
            &report(bundles()),
            &Target {
                team_id: "ABCDE12345".into(),
                kind: TeamKind::Personal,
                watch: WatchChoice::Remove,
                injected_dylibs: Vec::new(),
            },
        );
        // The Watch app and everything nested inside it go together.
        assert!(removed.blockers.is_empty());
        assert_eq!(removed.app_ids_required, 1);
        assert!(!removed.bundles.iter().any(|b| b.path.contains("Watch")));
        assert!(removed.consequences.iter().any(|c| c.contains("watchOS")));

        let signed = build(
            &report(bundles()),
            &Target {
                team_id: "ABCDE12345".into(),
                kind: TeamKind::Personal,
                watch: WatchChoice::Sign,
                injected_dylibs: Vec::new(),
            },
        );
        assert_eq!(signed.app_ids_required, 3);
    }

    #[test]
    /// An IPA needing more App IDs than a free team may register in seven days is refused before
    /// anything is reserved, rather than halfway through spending the budget.
    fn the_personal_team_app_id_budget_blocks_oversized_ipas() {
        let mut bundles = vec![bundle(
            "Payload/App.app",
            "Main app",
            "com.company.app",
            vec![],
        )];
        for index in 0..PERSONAL_APP_ID_BUDGET {
            bundles.push(bundle(
                &format!("Payload/App.app/PlugIns/E{index}.appex"),
                "Extension",
                &format!("com.company.app.e{index}"),
                vec![],
            ));
        }
        let plan = build(&report(bundles), &personal());
        assert!(plan.blockers.iter().any(|b| b.contains("App IDs")));
    }

    #[test]
    /// With no team selected or no main identifier to rewrite, the plan blocks rather than
    /// inventing a value and producing a build nobody asked for.
    fn a_missing_team_or_identifier_blocks_instead_of_inventing_one() {
        let plan = build(
            &report(vec![bundle("Payload/App.app", "Main app", "", vec![])]),
            &Target {
                team_id: String::new(),
                kind: TeamKind::Personal,
                watch: WatchChoice::Sign,
                injected_dylibs: Vec::new(),
            },
        );
        assert_eq!(plan.blockers.len(), 2);
        assert!(plan.new_main_identifier.is_empty());
    }
}
