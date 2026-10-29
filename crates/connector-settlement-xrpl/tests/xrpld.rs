//! The stand-alone `xrpld` test chain and the connector's XRPL client
//! against it (issue #1610). Each test spawns its own chain (ADR 0084).

use std::process::{Command, Stdio};
use std::time::Duration;

use connector_chain_rpc::xrpl::TxFate;
use connector_settlement_xrpl::test_support::{
    require_xrpld, Xrpld, IMAGE, OWNER_LABEL, STANDALONE_NETWORK_ID,
};
use serde_json::json;

const POLL: Duration = Duration::from_millis(50);

#[tokio::test]
async fn a_funded_account_reads_back_its_balance_through_the_transport_built_client() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();

    let wallet = chain.funded_wallet(&rpc, 1_000_000_000).await;

    let info = rpc
        .account_info(&wallet.address)
        .await
        .expect("account_info")
        .expect("the funded account exists");
    assert_eq!(info.balance_drops, 1_000_000_000);
    assert_eq!(info.owner_count, 0);
}

#[tokio::test]
async fn an_unfunded_account_does_not_exist() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let (address, _seed) = rpc.wallet_propose().await.expect("wallet_propose");
    assert_eq!(rpc.account_info(&address).await.unwrap(), None);
}

#[tokio::test]
async fn server_info_reports_the_configured_stand_alone_network_id() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let info = chain.rpc().server_info().await.expect("server_info");
    assert_eq!(info.network_id, Some(STANDALONE_NETWORK_ID));
}

#[tokio::test]
async fn a_transaction_that_passes_its_last_ledger_sequence_unlanded_is_reported_not_landed() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let payer = chain.funded_wallet(&rpc, 100_000_000).await;
    let payee = chain.funded_wallet(&rpc, 100_000_000).await;

    let account = rpc.account_info(&payer.address).await.unwrap().unwrap();
    let last_ledger_sequence = rpc.fee().await.unwrap().current_ledger_index + 2;
    // A sequence the account has not reached: the node holds it and never
    // applies it, so it cannot land.
    let submitted = rpc
        .sign_and_submit(
            json!({
                "TransactionType": "Payment",
                "Account": payer.address,
                "Destination": payee.address,
                "Amount": "1000000",
                "Sequence": account.sequence + 5,
                "LastLedgerSequence": last_ledger_sequence,
            }),
            &payer.seed,
        )
        .await
        .expect("submit");
    assert!(!submitted.hash.is_empty(), "{submitted:?}");

    // The test closes ledgers while the wait polls, as nothing else will.
    let closer = async {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            rpc.ledger_accept().await.expect("ledger_accept");
        }
    };
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            rpc.wait_for_tx(&submitted.hash, last_ledger_sequence, POLL),
            closer
        )
        .0
    })
    .await
    .expect("the wait is bounded by LastLedgerSequence and does not hang")
    .expect("wait_for_tx");

    assert_eq!(
        waited,
        TxFate::KnownNotLanded {
            last_ledger_sequence
        }
    );
    // And nothing moved.
    let payee_balance = rpc.account_info(&payee.address).await.unwrap().unwrap();
    assert_eq!(payee_balance.balance_drops, 100_000_000);
}

/// UNCONFIRMED fact (a) of #1596: repeated `ledger_accept` advances the
/// stand-alone chain's close time, at least a second per close, without
/// waiting for the wall clock.
#[tokio::test]
async fn repeated_ledger_accept_advances_the_close_time_faster_than_the_wall_clock() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    let close_time = || async {
        rpc.call("ledger", json!({ "ledger_index": "validated" }))
            .await
            .expect("ledger")["ledger"]["close_time"]
            .as_u64()
            .expect("close_time")
    };

    rpc.ledger_accept().await.unwrap();
    let before = close_time().await;
    let started = std::time::Instant::now();
    const ACCEPTS: u64 = 10;
    for _ in 0..ACCEPTS {
        rpc.ledger_accept().await.unwrap();
    }
    let advanced = close_time().await - before;
    eprintln!(
        "xrpld: {ACCEPTS} ledger_accepts advanced close time by {advanced} s in {:?}",
        started.elapsed()
    );
    assert!(
        advanced >= ACCEPTS,
        "{ACCEPTS} closes must move close time at least {ACCEPTS} s, moved {advanced} s"
    );
}

/// ADR 0084's line: a chain is "too heavy" for per-test spawning past 10 s
/// to its first answer or 1 GB. The numbers are printed for the record
/// (`--nocapture`); the 10 s half is an assertion, so a slower runner fails
/// loudly instead of quietly breaching the ADR.
#[tokio::test]
async fn startup_is_inside_adr_0084s_line() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let rpc = chain.rpc();
    for _ in 0..5 {
        chain.funded_wallet(&rpc, 100_000_000).await;
    }
    eprintln!(
        "xrpld: startup {:?}, memory after 5 funded accounts {}",
        chain.startup,
        chain.memory_usage()
    );
    assert!(
        chain.startup < Duration::from_secs(10),
        "xrpld took {:?} to its first server_info: past ADR 0084's 10 s line, \
         so XRPL belongs in the once-per-binary tier",
        chain.startup
    );
}

#[tokio::test]
async fn a_container_left_by_a_killed_run_is_cleared_by_the_next_spawn() {
    if !require_xrpld() {
        return;
    }
    // A process that has exited: its PID is no one's now.
    let mut dead = Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();

    let orphan = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--label",
            &format!("{OWNER_LABEL}={dead_pid}"),
            "--entrypoint",
            "sleep",
            IMAGE,
            "600",
        ])
        .stderr(Stdio::inherit())
        .output()
        .unwrap();
    assert!(orphan.status.success());
    let orphan = String::from_utf8_lossy(&orphan.stdout).trim().to_string();
    let running = |id: &str| {
        let out = Command::new("docker")
            .args(["ps", "-q", "--filter", &format!("id={id}")])
            .output()
            .unwrap();
        !out.stdout.is_empty()
    };
    assert!(running(&orphan));

    let _chain = Xrpld::spawn().await;

    assert!(!running(&orphan), "the orphan survived the next spawn");
}

#[tokio::test]
async fn dropping_the_chain_removes_its_container() {
    if !require_xrpld() {
        return;
    }
    let chain = Xrpld::spawn().await;
    let id = chain.container_id().to_string();
    let exists = || {
        let out = Command::new("docker")
            .args(["ps", "-aq", "--filter", &format!("id={id}")])
            .output()
            .unwrap();
        !out.stdout.is_empty()
    };
    assert!(exists());
    drop(chain);
    assert!(!exists());
}
