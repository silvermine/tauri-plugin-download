use serde::{Deserialize, Serialize};

/// Describes the physical or logical transport used to connect to the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionType {
   /// Connected via Wi-Fi.
   Wifi,

   /// Connected via Ethernet (wired).
   Ethernet,

   /// Connected via a cellular network (WWAN).
   Cellular,

   /// The connection type could not be determined.
   Unknown,
}

/// Information about the current network connection.
///
/// Combines cost/constraint flags with the physical [`ConnectionType`] to give callers
/// enough context to make download policy decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
   /// Whether data usage is billed or limited (e.g. mobile data plans, capped hotspots).
   pub metered: bool,

   /// Whether the connection is constrained — approaching or over its data limit,
   /// or background data usage is restricted.
   pub constrained: bool,

   /// The physical or logical transport used to connect to the network.
   pub connection_type: ConnectionType,
}
