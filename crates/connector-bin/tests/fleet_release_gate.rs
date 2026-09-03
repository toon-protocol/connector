//! Guards the connector's release/build pipeline: the release dispatch, the
//! candidate tags it publishes, the release handle shape, and the health
//! probe's coverage. [ADR
//! 0041](../../../docs/adr/0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md),
//! [ADR
//! 0055](../../../docs/adr/0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md)
//! and [ADR
//! 0068](../../../docs/adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md).
//!
//! Deliberately a SEPARATE harness from `devnet_configs_load.rs` rather than
//! more cases appended to it. That file asserts what the committed fleet
//! fixtures contain; this one asserts what the committed release *pipeline*
//! does, and the two are edited by different work for different reasons.
//!
//! ADR 0068 retired the promotion half of this gate: `promote-to-fleet.yml`
//! is gone, and with it every case that asserted its content (the deploy-
//! ordering gate, the apply-run verification, the fleet tag retag). Neither
//! devnet box deploys from this repository any more -- each pins the
//! connector by release handle in its OWN repo's `deploy/` bundle -- so there
//! is nothing left here for a promotion to gate. What survives is everything
//! upstream of that: the build stays one shared definition, the release
//! handle stays a dated ordinal, and the release workflow stays a human act.
//!
//! Most surviving cases are regression tests for something that actually
//! happened on 2026-08-16, and each one names it.
//!
//! The CodeQL cases at the end are the same kind of thing for a different
//! pipeline: `.github/workflows/codeql.yml` filters one query out of the
//! scan (#1235), and a filter is a list that only ever grows.

use std::collections::BTreeSet;

const PUBLISH_CONNECTOR_WORKFLOW: &str =
    include_str!("../../../.github/workflows/publish-connector-rust-image.yml");
const RELEASE_WORKFLOW: &str = include_str!("../../../.github/workflows/release-connector.yml");
const FLEET_HEALTH_WORKFLOW: &str = include_str!("../../../.github/workflows/fleet-health.yml");
const CODEQL_WORKFLOW: &str = include_str!("../../../.github/workflows/codeql.yml");
const CODEQL_CONFIG: &str = include_str!("../../../.github/codeql/codeql-config.yml");
const RELAY_SWAP_CONFIG: &str = include_str!("../../../infra/linode-relay/swap.config.json");
const RELAY_SWAP_OVERLAY: &str =
    include_str!("../../../infra/linode-relay/docker-compose.relay.swap.yml");

/// A `docker/metadata-action` `type=raw,value=<tag>` line, ignoring `#`
/// comments -- the workflow discusses `rust-release` at length in its
/// header, and a test keyed on the word alone would be asserting prose.
fn raw_metadata_tags(raw: &str) -> BTreeSet<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("type=raw,value="))
        .map(|rest| {
            rest.split(',')
                .next()
                .unwrap_or(rest)
                .trim()
                .trim_end_matches("}}")
                .to_string()
        })
        .collect()
}

/// THE regression test for the mistake this file exists to catch.
///
/// connector#990 shipped `type=raw,value=rust-release,enable={{is_default_branch}}`
/// here, which made `:rust-release` move on every green merge to `main`. Both
/// devnet boxes were then repointed to follow that tag under a label-scoped
/// Watchtower, so every green merge reached the live client edge on two
/// machines inside a minute, unvalidated.
///
/// ADR 0068 retired the mechanism that used to supervise this tag
/// (`promote-to-fleet.yml`) because neither box follows it from this repo any
/// more -- each node repo pins the connector image by release handle in its
/// own `deploy/` bundle. That makes this property MORE important, not less:
/// with no promotion gate left in this repo, `rust-release` reappearing here
/// would move a tag with no check in front of it at all.
#[test]
fn the_build_workflow_publishes_candidates_and_never_moves_the_promotion_tag() {
    let tags = raw_metadata_tags(PUBLISH_CONNECTOR_WORKFLOW);

    assert!(
        !tags.contains("rust-release"),
        "publish-connector-rust-image.yml pushes `rust-release` again. Nothing \
         in this repository supervises that tag any more (ADR 0068) -- a node \
         repo pins the connector by release handle in its own deploy/ bundle. \
         Re-adding it here would move a tag with no gate in front of it at \
         all. Tags found: {tags:?}"
    );

    // The candidate tags must still be published: a node repo pins one of
    // these (the immutable `rust-sha-` tag, or the release's `rust-<handle>`
    // alias) as its own reviewed change.
    assert!(
        tags.iter().any(|t| t.starts_with("rust-sha-")),
        "publish-connector-rust-image.yml no longer publishes an immutable \
         `rust-sha-` tag. A node repository pins exactly this shape to adopt \
         a build. Tags found: {tags:?}"
    );
}

/// The top-level `on:` keys of a workflow, by indentation, ignoring comments.
/// The workflow discusses its triggers at length in prose, so a test keyed
/// on the word `push` alone would be asserting a paragraph.
fn workflow_triggers(raw: &str) -> BTreeSet<String> {
    let mut triggers = BTreeSet::new();
    let mut inside = false;
    for line in raw.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !inside {
            inside = line == "on:";
            continue;
        }
        // Any further line at column 0 ends the block.
        if !line.starts_with(' ') {
            break;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent == 2 {
            if let Some(key) = trimmed.split(':').next() {
                triggers.insert(key.to_string());
            }
        }
    }
    triggers
}

/// THE regression test for ADR 0041 Decision 3, restated by ADR 0055 and
/// left standing by ADR 0068: a release is a human act.
///
/// `release-connector.yml` builds, versions and publishes a GitHub Release --
/// which is to say it is one `workflow_run:` away from firing on every green
/// merge. connector#990 was the one-line version of that mistake and it
/// reached both live devnet boxes within ~60s of every merge at the time.
/// `connector-rust` is still the client edge on both boxes today, even though
/// neither pulls its image from a tag this repo moves any more -- an
/// automatic trigger here would still mean an unreviewed image and release
/// on every merge, which is the churn this rule exists to prevent.
///
/// A `pull_request` trigger would be no better than a `push` one here.
#[test]
fn the_release_workflow_is_dispatch_only() {
    let triggers = workflow_triggers(RELEASE_WORKFLOW);
    let expected: BTreeSet<String> = ["workflow_dispatch".to_string()].into_iter().collect();

    assert_eq!(
        triggers, expected,
        "release-connector.yml is triggered by something other than a human \
         dispatch. Cutting a release and publishing a GitHub Release is a \
         deliberate act (ADR 0041 Decision 3, ADR 0055) and ANY automatic \
         trigger here turns it into churn on every merge -- the shape \
         connector#990 shipped once and was reverted."
    );
}

/// One build definition, shared with green main.
///
/// A release that ran its own `docker build` would be a second place to keep
/// in step with the Dockerfile, the amd64-only decision (#487's recorded
/// reversal) and the ADR 0009 refuses-without-a-config assertion — and it
/// would be the copy that runs rarely, so its drift would surface on a
/// release day, which is the worst available day for it.
#[test]
fn a_release_builds_through_the_same_workflow_a_green_main_does() {
    assert!(
        PUBLISH_CONNECTOR_WORKFLOW.contains("workflow_call:"),
        "publish-connector-rust-image.yml is no longer callable. \
         release-connector.yml calls it so a release and a green merge produce \
         a build from one definition."
    );
    assert!(
        RELEASE_WORKFLOW.contains("uses: ./.github/workflows/publish-connector-rust-image.yml"),
        "release-connector.yml no longer builds through \
         publish-connector-rust-image.yml. If it grew its own build step there \
         are now two build definitions, and only one of them runs on an \
         ordinary day."
    );
    assert!(
        !RELEASE_WORKFLOW.contains("docker/build-push-action"),
        "release-connector.yml builds an image itself. It must call \
         publish-connector-rust-image.yml instead — see both files' headers \
         and ADR 0055."
    );
}

/// The handle is a date and an ordinal, and it is not semver.
///
/// deploy/connector-rust/README.md's reasoning about the image tags binds
/// here too: no crate under `crates/` has a release process, every one is
/// `0.1.0`, and a semver series "would claim a stability contract the binary
/// hasn't earned". Someone eventually pins against a version number, and a
/// MAJOR that means nothing is worse than no number at all.
#[test]
fn the_release_handle_is_a_dated_ordinal_and_never_a_semver_series() {
    assert!(
        RELEASE_WORKFLOW.contains("date -u +%Y.%m.%d"),
        "release-connector.yml no longer cuts its handle from a UTC date. A \
         local date makes the series non-monotonic for anyone west of \
         Greenwich, and a handle that is not a date is a version number \
         wearing a disguise. See ADR 0055."
    );
    assert!(
        !RELEASE_WORKFLOW.contains("semantic-release"),
        "release-connector.yml reaches for a semver series. Every crate under \
         `crates/` is 0.1.0 with no release process; a version here would \
         claim a stability contract the binary has not earned. See ADR 0055 \
         and deploy/connector-rust/README.md."
    );
}

/// The chain-provider fields the maker refuses to boot without, mirrored from
/// `SWAP_REQUIRED_PROVIDER_FIELDS.evm` in toon-protocol/swap
/// `packages/swap/src/swap-node.ts`. Enforced by `validateChainProviderEntry`,
/// which throws `SwapNodeStartError('INVALID_CONFIG', ...)` naming the missing
/// setting.
///
/// A mirror, not a derivation -- this repo cannot import the maker's schema.
/// That is precisely why `fleet-health.yml`'s config-compat gate boots the
/// real image against this real file rather than trusting a list like this
/// one: a key added in `swap` will not appear here on its own. This case
/// catches the cheaper failure, an edit to the config in THIS repo that drops
/// a field the maker needs, and it catches it in CI instead of on the box.
const SWAP_REQUIRED_EVM_PROVIDER_FIELDS: &[&str] = &[
    "chainId",
    "rpcUrl",
    "registryAddress",
    "tokenAddress",
    // Leg A. The one swap#134 added and the box did not have.
    "tokenNetworkAddress",
    // Leg B, the EIP-712 `verifyingContract`. A DIFFERENT contract from
    // `tokenNetworkAddress`; neither defaults to the other (swap#133).
    "channelAddress",
];

/// The outage of 2026-08-16, as a test.
///
/// swap#134 made `chainProviders[].tokenNetworkAddress` required with no
/// default. It merged green, `swap:release` moved, the relay box's Watchtower
/// recreated `swap-node` within ~60s, and the maker crash-looped on
/// `[INVALID_CONFIG] chainProviders[0].tokenNetworkAddress MUST be a non-empty
/// string` -- because this file is BIND-MOUNTED at
/// `/app/config/swap.config.json` and is not part of the image, so no image
/// build ever saw it. A human added the key to the live copy to stop the loop.
///
/// Until that value was brought back here, a redeploy from the committed tree
/// would have reproduced the outage exactly. This asserts it stays.
#[test]
fn the_committed_maker_config_satisfies_every_field_the_maker_requires() {
    let config: serde_json::Value = serde_json::from_str(RELAY_SWAP_CONFIG).expect(
        "infra/linode-relay/swap.config.json is not valid JSON -- the maker reads it verbatim",
    );

    let providers = config
        .get("chainProviders")
        .and_then(|p| p.as_array())
        .expect("swap.config.json has no `chainProviders` array");
    assert!(
        !providers.is_empty(),
        "swap.config.json's `chainProviders` is empty -- the maker refuses to \
         boot without an entry for every chain a `swapPair` targets."
    );

    for (i, provider) in providers.iter().enumerate() {
        let chain_type = provider
            .get("chainType")
            .and_then(|c| c.as_str())
            .unwrap_or_else(|| panic!("chainProviders[{i}] has no `chainType`"));
        // Only the EVM required-field set is mirrored here; the fleet has run
        // nothing else in this file. A solana/mina provider appearing without
        // its own mirrored list would pass vacuously, so say so rather than
        // let it.
        assert_eq!(
            chain_type, "evm",
            "chainProviders[{i}] is `{chain_type}`, and this test only mirrors \
             the maker's EVM required-field set. Add the matching list from \
             swap's SWAP_REQUIRED_PROVIDER_FIELDS before committing a \
             non-EVM provider, or this case passes it without checking anything."
        );

        for field in SWAP_REQUIRED_EVM_PROVIDER_FIELDS {
            let value = provider.get(*field).and_then(|v| v.as_str());
            assert!(
                value.is_some_and(|v| !v.is_empty()),
                "infra/linode-relay/swap.config.json `chainProviders[{i}]` is \
                 missing a non-empty `{field}`. The maker validates this before \
                 allocating any resource and exits INVALID_CONFIG naming the \
                 setting -- and because this file is bind-mounted rather than \
                 baked into the image, that lands as a crash-loop on the relay \
                 box roughly 60 seconds after `swap:release` next moves. This \
                 is the 2026-08-16 outage."
            );
        }
    }
}

/// Environment variables `docker-compose.relay.swap.yml` supplies to the
/// maker. The config FILE is only half the box's configuration: without
/// `SWAP_AUTOGEN_IDENTITY`, `swap.config.json` alone fails
/// `[INVALID_CONFIG] SwapNodeConfig: one of mnemonic or secretKey is required`,
/// because swap#127 made the maker self-generate and persist its own BIP-39
/// mnemonic to `statePath` on first boot rather than read a committed one.
const RELAY_SWAP_SERVICE_ENV: &[&str] = &["SWAP_AUTOGEN_IDENTITY"];

/// The config-compatibility gate boots the `:release` image against the
/// committed `swap.config.json`, and it can only be trusted if what it boots
/// is what the BOX boots. The first run of this gate proved that is not
/// automatic: booting the file by itself failed on a missing identity that
/// the box never misses, because the overlay supplies it as an environment
/// variable rather than a config key.
///
/// So `fleet-health.yml`'s `config-compat` job reproduces the SERVICE: the
/// file, plus this environment, plus a writable state mount. This case is
/// what stops the two descriptions drifting: a variable added to the overlay
/// and not to the gate would leave the gate quietly validating a
/// configuration the box does not run, which is a worse failure than having
/// no gate, because it reads as a pass.
#[test]
fn the_config_compat_gate_reproduces_the_makers_committed_service_environment() {
    for var in RELAY_SWAP_SERVICE_ENV {
        assert!(
            RELAY_SWAP_OVERLAY.contains(var),
            "docker-compose.relay.swap.yml no longer sets `{var}`. If the \
             maker genuinely no longer needs it, drop it from \
             RELAY_SWAP_SERVICE_ENV and from both config-compat gates -- do \
             not leave the gates passing a variable the service does not have."
        );
        assert!(
            FLEET_HEALTH_WORKFLOW.contains(&format!("-e {var}=")),
            "fleet-health.yml's config-compat job does not pass `{var}` to the \
             image, but docker-compose.relay.swap.yml supplies it to the live \
             service. The gate would be booting a configuration the box never \
             runs -- and it would FAIL on a config the box is perfectly happy \
             with, which is how a gate gets disabled."
        );
    }

    // The writable state mount is the other half of the same point:
    // `SWAP_AUTOGEN_IDENTITY` persists the generated mnemonic to `statePath`,
    // so without somewhere to write it the boot is not the box's boot.
    assert!(
        FLEET_HEALTH_WORKFLOW.contains(":/app/state"),
        "fleet-health.yml's config-compat job no longer mounts a writable \
         /app/state. The maker persists its self-generated identity to \
         `statePath` there (swap.config.json), so a boot without it is not \
         the boot the box performs."
    );
}

/// Every service a box's Watchtower can recreate unattended, as observed live
/// on both boxes on 2026-08-16 (`docker ps --filter
/// label=com.centurylinklabs.watchtower.enable=true`).
///
/// `fleet-health.yml` DISCOVERS this set at runtime rather than reading a list
/// -- that is deliberate, so a service labelled on the box but not committed
/// here is still probed. This constant exists for the opposite direction: to
/// fail the build if a probe arm is deleted for a service that is known to be
/// running under that label, which discovery cannot notice because a missing
/// arm only reports at 03:00 on a cron.
const WATCHTOWER_MANAGED_SERVICES: &[&str] = &[
    // relay + store boxes
    "connector-rust",
    // relay box
    "relay",
    "swap-node",
    // store box
    "store",
];

/// A Watchtower-managed service with no serving probe is the gap toon-meta#403
/// filed and never built ("Watchtower does no health-gating; a bad image
/// auto-deploys and the container just crash-loops"). `fleet-health.yml`
/// answers an unknown service with a FAIL rather than a skip, so a new service
/// cannot be opted into auto-redeploy silently -- but a probe arm DELETED for
/// a service that is still labelled would only surface as a cron failure on
/// some later night. This asserts it at build time instead.
#[test]
fn fleet_health_defines_a_probe_for_every_watchtower_managed_service() {
    for service in WATCHTOWER_MANAGED_SERVICES {
        // The probe table is a shell `case` over the compose service name.
        // `relay|store` share one arm, so match either spelling.
        let has_arm = FLEET_HEALTH_WORKFLOW.contains(&format!("\n              {service})"))
            || FLEET_HEALTH_WORKFLOW.contains(&format!("{service}|"))
            || FLEET_HEALTH_WORKFLOW.contains(&format!("|{service})"));
        assert!(
            has_arm,
            "fleet-health.yml has no probe arm for `{service}`, which runs \
             under the Watchtower enable label on a live box. Without one the \
             workflow reports it as `NO PROBE DEFINED` on every run, which is \
             a standing failure rather than a check. Add the arm, or remove \
             the service from WATCHTOWER_MANAGED_SERVICES if it is genuinely \
             no longer auto-deployed."
        );
    }

    // The unknown-service arm itself: without it a newly labelled service
    // would be silently unprobed, which is the failure mode this whole file
    // is about.
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("NO PROBE DEFINED"),
        "fleet-health.yml no longer fails on a Watchtower-managed service it \
         has no probe for. A skip there means a service can be opted into \
         unattended redeploy with nothing checking that it serves."
    );
}

/// The health workflow is only worth having if a human hears it. toon-meta#403
/// asked for "an ... external check should alert" and the alert is the half
/// that was never specified; a red tick on a 15-minute cron is not one.
#[test]
fn an_unhealthy_fleet_opens_a_labelled_issue() {
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("gh issue create"),
        "fleet-health.yml no longer opens an issue on failure. Detection \
         nobody sees is what this workflow exists to replace -- the 2026-08-16 \
         maker crash-loop was found by a human happening to look."
    );
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("--label \"needs:human\""),
        "fleet-health.yml's alert no longer carries `needs:human`. That is the \
         org's existing swept human queue (toon-meta#347); dropping it puts \
         the alert in a channel nobody is already reading."
    );
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("issues: write"),
        "fleet-health.yml no longer requests `issues: write`, so its alert \
         step cannot open anything and the failure is silent again."
    );
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("gh issue close"),
        "fleet-health.yml no longer closes its rolling alert on recovery. The \
         issue's open/closed state is meant to BE the fleet's current verdict; \
         an alert that never closes has to be read and dismissed by hand every \
         time, which is how a monitor stops being read."
    );
}

/// The `query-filters:` entries of a CodeQL config, as `(kind, id)` pairs in
/// file order, ignoring comments. The config discusses the query it excludes
/// at length, so a test keyed on the id alone would be asserting a paragraph.
fn codeql_query_filters(raw: &str) -> Vec<(String, String)> {
    let mut filters = Vec::new();
    let mut inside = false;
    let mut kind: Option<String> = None;
    for line in raw.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !inside {
            inside = line == "query-filters:";
            continue;
        }
        // Any further line at column 0 ends the block.
        if !line.starts_with(' ') {
            break;
        }
        let trimmed = line.trim_start();
        if let Some(entry) = trimmed.strip_prefix("- ") {
            kind = Some(entry.trim_end_matches(':').to_string());
        } else if let Some(id) = trimmed.strip_prefix("id:") {
            let kind = kind.clone().expect(
                "an `id:` under `query-filters:` with no `- exclude:`/`- include:` above it",
            );
            filters.push((kind, id.trim().to_string()));
        }
    }
    filters
}

/// The column-0 keys of a YAML document, ignoring comments.
fn top_level_keys(raw: &str) -> BTreeSet<String> {
    raw.lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with(' ') && !line.trim().is_empty())
        .filter_map(|line| line.split(':').next().map(str::to_string))
        .collect()
}

/// `rust/hard-coded-cryptographic-value` matches on a parameter NAME, and
/// every claim fixture here has one called `nonce` -- a monotonic counter
/// the counterparty signs over (ADR 0053), not a secret. 463 alerts on
/// `main`, and #1228 needed four dismissed by hand to go green. The fix is
/// a config that excludes that one query (#1235); the risk is that a config
/// which can exclude one query can exclude a second, or carve a directory
/// out with `paths-ignore`, and neither would show up anywhere but a
/// quieter alert count. This pins the exclusion set to exactly one id and
/// the config to the two keys it needs.
#[test]
fn codeql_runs_the_committed_config_and_excludes_exactly_one_query() {
    assert!(
        CODEQL_WORKFLOW.contains("config-file: ./.github/codeql/codeql-config.yml"),
        "codeql.yml no longer passes `.github/codeql/codeql-config.yml` to \
         `github/codeql-action/init`. Without it the scan runs unfiltered and \
         the 463 claim-nonce alerts come back, which is the state #1235 was \
         filed against."
    );

    let filters = codeql_query_filters(CODEQL_CONFIG);
    let expected = vec![(
        "exclude".to_string(),
        "rust/hard-coded-cryptographic-value".to_string(),
    )];
    assert_eq!(
        filters, expected,
        "codeql-config.yml's `query-filters` is not exactly one exclusion of \
         `rust/hard-coded-cryptographic-value`. Every other open rule -- \
         `rust/cleartext-logging`, `actions/missing-workflow-permissions` -- \
         is real, and a new alert of an excluded shape is a question about \
         this config, not a reason to widen it. If a second exclusion is \
         genuinely warranted, it gets its own rationale comment AND this \
         expected list changes with it."
    );

    let keys = top_level_keys(CODEQL_CONFIG);
    let allowed: BTreeSet<String> = ["name", "query-filters"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let extra: Vec<&String> = keys.difference(&allowed).collect();
    assert!(
        extra.is_empty(),
        "codeql-config.yml grew top-level key(s) {extra:?}. `paths` and \
         `paths-ignore` silence a directory rather than a query, `queries` \
         and `packs` change the suite away from the one default setup ran, \
         and `disable-default-queries` turns the scan off; none of those is \
         the one-query filter #1235 asked for."
    );

    // The switch has a human step no workflow performs -- an admin turns
    // default setup off, or GitHub refuses this workflow's uploads -- and
    // the header is where the next person finds that out.
    assert!(
        CODEQL_WORKFLOW.contains("code-scanning/default-setup"),
        "codeql.yml's header no longer names the default-setup switch. \
         GitHub rejects SARIF from an advanced workflow while default setup \
         is enabled, so a reader seeing the `analyze` step fail on upload \
         needs to be told it is a settings flip, not a broken scan."
    );
}

/// The move from default setup was meant to change one query, not the
/// coverage. Default setup's own uploads were categorised
/// `/language:actions`, `/language:javascript-typescript`,
/// `/language:python` and `/language:rust` (its six configured language
/// names are four extractors), on a weekly schedule plus every push and
/// PR. Dropping a language from the matrix would be a coverage cut that
/// looks like a tidy-up.
#[test]
fn codeql_covers_every_analysis_default_setup_ran() {
    for language in ["actions", "javascript-typescript", "python", "rust"] {
        assert!(
            CODEQL_WORKFLOW.contains(&format!("- language: {language}\n")),
            "codeql.yml's matrix no longer analyses `{language}`, which \
             default setup did. The switch to an advanced setup (#1235) was \
             a query filter, not a coverage change."
        );
    }
    assert!(
        !CODEQL_WORKFLOW.contains("build-mode: autobuild")
            && !CODEQL_WORKFLOW.contains("build-mode: manual"),
        "codeql.yml asks CodeQL to build something. Default setup analysed \
         Rust with `build-mode: none`, and the other three languages have no \
         other mode; a build here is a second Rust compile on every PR that \
         the scan does not need."
    );

    let triggers = workflow_triggers(CODEQL_WORKFLOW);
    for trigger in ["push", "pull_request", "schedule"] {
        assert!(
            triggers.contains(trigger),
            "codeql.yml no longer runs on `{trigger}` (it runs on {triggers:?}). \
             Default setup ran on every push, every PR and a weekly schedule; \
             the schedule is what catches a query-pack update against an \
             unchanged `main`."
        );
    }
}

/// The two outages of 2026-08-28, as a test.
///
/// `fleet-health.yml` checked container state and `GET /ilp/identity`, and
/// both stayed green through two multi-hour failures of a FORWARDED route:
/// `T01 peer unreachable` on `g.toon.relay.{store,gas}` from 10:26Z to 17:20Z,
/// and `T00 ... would not report the claim state of channel FDi2TCT9...` on
/// `g.toon.store.relay` from 01:27Z to 13:45Z, from an SPL mint cutover. Both
/// were found by a human sending a job by hand.
///
/// This asserts the workflow still asks the question those probes could not.
/// It is a text assertion because that is what this file can hold a workflow
/// to; the cross-check's own logic is tested where it lives, by
/// `tools/ci/fleet-peering-crosscheck.py --self-test`, which is why the
/// wiring of that self-test is asserted below rather than only its existence.
#[test]
fn fleet_health_still_cross_checks_a_forwarded_route() {
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("tools/ci/fleet-peering-crosscheck.py --self-test"),
        "fleet-health.yml no longer runs the cross-check's self-test before \
         trusting it against the live fleet. Every FAIL branch in that script \
         is unreachable from a healthy fleet, so the self-test is the only \
         thing that ever executes them -- without it the job is a green tick \
         over a monitor nobody has proven can go red."
    );
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("tools/ci/fleet-peering-crosscheck.py /tmp/peering.tsv"),
        "fleet-health.yml no longer cross-checks the fleet's self-descriptions \
         against each other. That is the only free check that catches an SPL \
         mint cutover on one box and not the other (2026-08-28 01:27Z): on \
         Solana the channel PDA is seeded with the mint, so the change moves \
         every channel id the two nodes share and the far side is asked about \
         a channel it has never heard of."
    );
    assert!(
        FLEET_HEALTH_WORKFLOW
            .contains("needs: [probe, config-compat, peering-crosscheck, paid-probe]"),
        "fleet-health.yml's alert job no longer waits on the peering jobs, so \
         a fleet that disagrees with itself would go green. The alert is the \
         half that makes detection worth anything."
    );
}

/// The paid probe is the only check here that spends, and it must stay
/// dispatch-only and off by default.
///
/// Not because spending is wrong -- it is the only thing that proves a packet
/// crosses a peering, and an unpaid request to a forwarded prefix returns
/// payment terms the near node answers alone, which proves nothing. It is
/// because arming a spend on a 15-minute cron is an operator's decision about
/// their own channels' balance, not a reviewer's.
#[test]
fn the_paid_probe_is_off_unless_a_human_asks_for_it() {
    assert!(
        FLEET_HEALTH_WORKFLOW.contains("default: 'off'"),
        "fleet-health.yml's `paid_probe` input no longer defaults to `off`. \
         Every scheduled run would then spend from the relay box's peer \
         channels -- roughly $0.20/day at the 15-minute cron -- and drain them \
         until someone funds them. Arming it is the operator's call; see \
         docs/operators/fleet-release-and-health.md."
    );
    assert!(
        FLEET_HEALTH_WORKFLOW
            .contains("if: always() && github.event_name == 'workflow_dispatch' && inputs.paid_probe != 'off'"),
        "fleet-health.yml's paid probe is no longer dispatch-gated. A schedule \
         trigger supplies no `inputs`, so a gate on the input alone is not \
         enough to keep a cron from spending."
    );
    assert!(
        !FLEET_HEALTH_WORKFLOW.contains("secrets.CI_WALLET")
            && !FLEET_HEALTH_WORKFLOW.contains("PRIVATE_KEY"),
        "fleet-health.yml now names a wallet secret. The paid probe deliberately \
         needs none: `connector send` drives the relay's own `POST /packets`, so \
         the money comes from the peer channel being tested and the only \
         credential is the operator write key already on that box. A funded key \
         in CI is a new class of secret and must be proposed to the operator, \
         not introduced by a workflow edit."
    );
}
