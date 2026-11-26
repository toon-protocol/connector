use std::path::PathBuf;

use crate::address::AddressRefusal;
use crate::batch_settlement::BatchSettlementRefusal;
use crate::client_channel_asset::ClientChannelAssetRefusal;
use crate::config::ConfigFileRefusal;
use crate::controller_keys::ControllerKeysRefusal;
use crate::cross_table::{CrossTableRefusal, PeeringAdmissionRefusal};
use crate::denomination::DenominationRefusal;
use crate::identity::ClientIdentityRefusal;
use crate::node::NodeRefusal;
use crate::operator::OperatorRefusal;
use crate::pay_channel::PayChannelRefusal;
use crate::peer::PeerRefusal;
use crate::peer_channel::PeerChannelRefusal;
use crate::peering_asset::PeeringAssetRefusal;
use crate::route::RouteRefusal;
use crate::secret::SignerRefusal;
use crate::settlement::SettlementRefusal;
use crate::zk_note::ZkNoteRefusal;

use thiserror::Error;

/// Every way [`crate::Config::load`] can fail.
///
/// Each variant names the offending field and, where useful, the value the
/// operator wrote, so the error is actionable without opening the source.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config file at {path} is not valid TOML: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    /// A refusal of the `[settlement]` tables, defined beside them in
    /// `settlement.rs`. Each table's refusals compose into this one type the
    /// same way (issue #1690), so the binary still handles one error.
    #[error(transparent)]
    Settlement(#[from] SettlementRefusal),

    /// A refusal of the `batch settlement` rules, defined beside them in `batch_settlement.rs`.
    #[error(transparent)]
    BatchSettlement(#[from] BatchSettlementRefusal),

    /// A refusal of the `client channel asset` rules, defined beside them in `client_channel_asset.rs`.
    #[error(transparent)]
    ClientChannelAsset(#[from] ClientChannelAssetRefusal),

    /// A refusal of the `controller keys` rules, defined beside them in `controller_keys.rs`.
    #[error(transparent)]
    ControllerKeys(#[from] ControllerKeysRefusal),

    /// A refusal of the `denomination` rules, defined beside them in `denomination.rs`.
    #[error(transparent)]
    Denomination(#[from] DenominationRefusal),

    /// A refusal of the `identity` rules, defined beside them in `identity.rs`.
    #[error(transparent)]
    ClientIdentity(#[from] ClientIdentityRefusal),

    /// A refusal of the `node` rules, defined beside them in `node.rs`.
    #[error(transparent)]
    Node(#[from] NodeRefusal),

    /// A refusal of the `operator` rules, defined beside them in `operator.rs`.
    #[error(transparent)]
    Operator(#[from] OperatorRefusal),

    /// A refusal of the `pay channel` rules, defined beside them in `pay_channel.rs`.
    #[error(transparent)]
    PayChannel(#[from] PayChannelRefusal),

    /// A refusal of the `peer channel` rules, defined beside them in `peer_channel.rs`.
    #[error(transparent)]
    PeerChannel(#[from] PeerChannelRefusal),

    /// A refusal of the `peering asset` rules, defined beside them in `peering_asset.rs`.
    #[error(transparent)]
    PeeringAsset(#[from] PeeringAssetRefusal),

    /// A refusal of the `peer` rules, defined beside them in `peer.rs`.
    #[error(transparent)]
    Peer(#[from] PeerRefusal),

    /// A refusal of the `route` rules, defined beside them in `route.rs`.
    #[error(transparent)]
    Route(#[from] RouteRefusal),

    /// A refusal of the `[signer]` rules, defined beside them in `secret.rs`.
    #[error(transparent)]
    Signer(#[from] SignerRefusal),

    /// A refusal of the `zk note` rules, defined beside them in `zk_note.rs`.
    #[error(transparent)]
    ZkNote(#[from] ZkNoteRefusal),

    /// The peering-admission refusals, mapped from the domain's rule set in `cross_table.rs`.
    #[error(transparent)]
    PeeringAdmission(#[from] PeeringAdmissionRefusal),

    /// The rules that read more than one parsed table, defined in `cross_table.rs`.
    #[error(transparent)]
    CrossTable(#[from] CrossTableRefusal),

    /// The refusals of reading the file itself (removed keys, bounds, the bind address and the
    /// socks proxy), defined beside the loader in `config.rs`.
    #[error(transparent)]
    File(#[from] ConfigFileRefusal),

    /// An ILP address that does not parse, raised by `node.rs` and `route.rs`, defined in `address.rs`.
    #[error(transparent)]
    Address(#[from] AddressRefusal),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table's refusals compose transparently (issue #1690): wrapping a
    /// refusal in the public error changes neither its message nor what it
    /// is, so the table module owns the message and the binary sees one type.
    macro_rules! transparent {
        ($($name:ident: $wrapper:ident($refusal:expr);)*) => {$(
            #[test]
            fn $name() {
                let refusal = $refusal;
                let message = refusal.to_string();
                let error = ConfigError::from(refusal);
                assert_eq!(error.to_string(), message);
                assert!(matches!(error, ConfigError::$wrapper(_)));
            }
        )*};
    }

    transparent! {
        a_batch_settlement_refusal_is_the_config_error_it_is_wrapped_in:
            BatchSettlement(BatchSettlementRefusal::BatchSettlementZeroMinimumSponsoredDeposit);
        a_controller_keys_refusal_is_the_config_error_it_is_wrapped_in:
            ControllerKeys(ControllerKeysRefusal::ControllerKeysFileInvalid {
                path: PathBuf::from("keys.toml"),
                message: "bad".to_owned(),
            });
        a_denomination_refusal_is_the_config_error_it_is_wrapped_in:
            Denomination(DenominationRefusal::RateGuardsMissing);
        a_client_identity_refusal_is_the_config_error_it_is_wrapped_in:
            ClientIdentity(ClientIdentityRefusal::ClientIdentityIdEmpty);
        a_node_refusal_is_the_config_error_it_is_wrapped_in:
            Node(NodeRefusal::NodeNoAddresses);
        an_operator_refusal_is_the_config_error_it_is_wrapped_in:
            Operator(OperatorRefusal::OperatorNoWriteKeys);
        a_pay_channel_refusal_is_the_config_error_it_is_wrapped_in:
            PayChannel(PayChannelRefusal::PayChannelDuplicatePeer { peer_id: "p".to_owned() });
        a_peer_channel_refusal_is_the_config_error_it_is_wrapped_in:
            PeerChannel(PeerChannelRefusal::PeerChannelVoucherSignerMissing {
                peer_id: "p".to_owned(),
            });
        a_peering_asset_refusal_is_the_config_error_it_is_wrapped_in:
            PeeringAsset(PeeringAssetRefusal::PeeringTokenNotDeclared {
                peer_id: "p".to_owned(),
                asset: "a".to_owned(),
            });
        a_peer_refusal_is_the_config_error_it_is_wrapped_in:
            Peer(PeerRefusal::PeerIdEmpty);
        a_route_refusal_is_the_config_error_it_is_wrapped_in:
            Route(RouteRefusal::MissingApex);
        a_signer_refusal_is_the_config_error_it_is_wrapped_in:
            Signer(SignerRefusal::SignerKmsIdEmpty);
        a_zk_note_refusal_is_the_config_error_it_is_wrapped_in:
            ZkNote(ZkNoteRefusal::ZkNoteViaSocksProxyWithoutProxy);
    }
}
