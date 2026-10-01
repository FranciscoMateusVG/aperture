# Local managed-agent package

Server, team-control and boot resolve runtime outputs relative to their **actual executable**:

```
<package>/bin/aperture-server
<package>/bin/aperture-team-control
<package>/bin/aperture-boot
<package>/mcp-server/{package.json,dist/,node_modules/}
<package>/mcp-server-sentry/{package.json,dist/src/index.js,node_modules/}
```

The catalog/template source and a mission repository are separate inputs, not a fallback for missing runtime JS. Managed Claude pins the sibling boot, bus, hub-client and Sentry entries through preflight/publication/spawn/release/the boot exec gate. Codex uses the same bus/Sentry layout. Missing leaves, drift or obsolete `dist/index.js` Sentry layout fail closed. The gate exports the hub-client path from that same pinned package, not the compiler checkout. tmux creation uses the session-only `aperture:` selector.

## Existing canonical MCP route

`APERTURE_TEAM_CONTROL_BIN` selects an absolute **regular file**; absent an override, MCP selects `~/.aperture/bin/aperture-team-control`. For that existing route, `~/.aperture` is the package root. Installation therefore copies the matching control AND boot binaries, **both whole MCP directories** (including their dependencies), never just control. Use independent copies (no symlinks or hardlinks for native binaries/entry leaves, nlink must remain 1). Normal packaged dependency layout is preserved, not relinked or pruned. The same release build supplies all three binaries.

Before installation, compare source hashes/modes and the destination copy, preserve previous binaries and old package trees, and ensure no managed launch is in flight. Never remove old packages while existing app-servers or inbox monitors reference them. Checkpoints and private launch records stay untouched; the change does not migrate or retry existing attempts. Updating mutable canonical assets while an older managed seat is using them can make its pin checks refuse: finish that seat first, or keep an explicitly selected versioned package for its lifecycle. This is the existing personal-local distribution, **not** certification of E1 immutable releases or dependency provenance.

Delivery evidence separates source/tests, installed bytes, server selection, and a real managed Claude + QA roundtrip. A successfully built helper is not proof that a team launched. Coordinator/hub processes are not restarted to update these files. A web-server-only native shutdown/reopen is separate if the packaged server itself is updated.
