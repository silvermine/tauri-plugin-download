use windows::Networking::Connectivity::NetworkInformation;

use crate::{ConnectionStatus, ConnectionType, Result};

/// [`NetworkCostType`](https://learn.microsoft.com/en-us/uwp/api/windows.networking.connectivity.networkcosttype) ordinals.
const NETWORK_COST_FIXED: i32 = 2;
const NETWORK_COST_VARIABLE: i32 = 3;

/// [`IanaInterfaceType`](https://www.iana.org/assignments/ianaiftype-mib/ianaiftype-mib) values.
const IANA_ETHERNET: u32 = 6;
const IANA_WIFI: u32 = 71;
const IANA_WWAN_PP: u32 = 243;
const IANA_WWAN_PP2: u32 = 244;

/// Queries the current connection status on Windows.
///
/// Uses [`ConnectionCost`](https://learn.microsoft.com/en-us/uwp/api/windows.networking.connectivity.connectioncost)
/// for metered/constrained flags, and
/// [`IsWwanConnectionProfile`](https://learn.microsoft.com/en-us/uwp/api/windows.networking.connectivity.connectionprofile.iswwanconnectionprofile) /
/// [`IsWlanConnectionProfile`](https://learn.microsoft.com/en-us/uwp/api/windows.networking.connectivity.connectionprofile.iswlanconnectionprofile)
/// to determine the physical connection type.
pub(crate) fn connection_status() -> Result<ConnectionStatus> {
   let profile = NetworkInformation::GetInternetConnectionProfile()?;
   let cost = profile.GetConnectionCost()?;

   let metered = matches!(
      cost.NetworkCostType()?.0,
      NETWORK_COST_FIXED | NETWORK_COST_VARIABLE
   );
   let constrained = metered && (cost.ApproachingDataLimit()? || cost.OverDataLimit()?);

   let connection_type = if profile.IsWwanConnectionProfile()? {
      ConnectionType::Cellular
   } else if profile.IsWlanConnectionProfile()? {
      ConnectionType::Wifi
   } else {
      iana_interface_type(&profile).unwrap_or(ConnectionType::Unknown)
   };

   Ok(ConnectionStatus {
      metered,
      constrained,
      connection_type,
   })
}

/// Maps the adapter's IANA interface type to a [`ConnectionType`].
fn iana_interface_type(
   profile: &windows::Networking::Connectivity::ConnectionProfile,
) -> Option<ConnectionType> {
   let iana_type = profile.NetworkAdapter().ok()?.IanaInterfaceType().ok()?;

   Some(match iana_type {
      IANA_ETHERNET => ConnectionType::Ethernet,
      IANA_WIFI => ConnectionType::Wifi,
      IANA_WWAN_PP | IANA_WWAN_PP2 => ConnectionType::Cellular,
      _ => ConnectionType::Unknown,
   })
}
