# Safety boundaries

Wayline leaves routing policy ownership with GL.iNet.

The monitor can read route results and send bounded diagnostic packets.
It cannot change route tables.
It binds the numeric interface index, source address, and mark.
It rejects a route that returns another interface.
No unbound fallback exists.

Measurements and direct WAN checks start disabled.
Direct WAN checks require separate consent through the settings UI.
Existing policy must permit the WAN path.
Wayline never adds a bypass exception.

The RPC bridge requires an active root administrator session.
It exposes six fixed methods.
The bridge cannot invoke arbitrary native modules or commands.
Rust validates every control request.
The frontend provides no security boundary.

Settings writes use a private temporary file, synchronization, and atomic rename.
Expected revisions reject stale UI writes.
Repeated unchanged settings do not rewrite the configuration.
Service reconciliation changes only Wayline's own service.

Exit selection requires the exact firmware contract and equivalent active traffic rules.
The selected exit needs fresh successful measurements and no recent loss.
The helper preserves every native group member and the separate default tunnel group.
It reads the complete native order after a change.

The native API has no compare-and-swap operation.
A factory UI change can race with a Wayline request.
Wayline detects revision changes before the call and checks the result afterward.
It does not blindly retry or restore an assumed previous order.
An uncertain result requires a refresh.

Update checks authenticate a signed manifest before trusting package metadata.
Installation requires matching architecture, firmware, package identity, size, and SHA-256.
An uncertain installation blocks automatic retries during the current boot.
Power loss and package-manager failure can still require recovery.
No software can guarantee failure-free router operation.

Physical router checks remain necessary for native RPC, UI authentication, tunnel selection, and firmware upgrades.
The isolated WireGuard lab checks the measurement core.
It does not replace those hardware checks.
