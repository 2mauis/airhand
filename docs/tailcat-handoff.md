# Tailcat desktop handoff

This review branch keeps the exact PR #17 source baseline (`0740a91ac489f26e28b4417bc763e215600895b3`), the bilingual fork-intent README changes from PR #18, and the [original Claude Code design Markdown](specs/tailcat-direct-mvp.md). It is an archive for handoff, not a development base or a declaration that the feature is implemented.

## Scope and state

- Goal: a Tailcat-managed, mutually authenticated AirHand desktop session with no independent AirHand relay or signaling server. Keep Tailcat-native DERP/STUN. Do not confuse native DERP bootstrap with an application-data relay; the production data fallback policy has not been decided.
- PR #17 was built and exercised on two macOS devices using the existing AirHand relay mode. That result does **not** validate Tailcat desktop integration. The temporary AirHand test relay was stopped and its listener removal verified.
- On those same two devices, Tailcat v0.7.0 was tested separately. Both peers reported UDP direct, 100/100 synthetic 1024-byte request/reply exchanges succeeded, and the DERP data-frame counter remained 6 → 6 over 6.83 seconds after reaching direct. A later authenticated repeat yielded 8/8 and DERP data 4 → 4; an independent unlisted test key was rejected with no data flow. These are library-level results, not AirHand keyboard, mouse, or screen results.
- The AirHand production helper control entry is still a stub at this baseline. Desktop session setup still depends on the relay's authorization and stream matching. There is no usable Tailcat desktop build, new feature implementation, or post-integration CI result in this archive.

## Continue from here

1. Verify the current target `develop` ref and required upstream ancestry in the development environment. Create the new development branch from the confirmed synchronized base, not this review branch. Import the approved design verbatim and port only relevant baseline changes if needed.
2. Implement the narrow pair of macOS peers described by the approved design. Preserve per-session user consent, the allowlist, local-only secrets and fail-closed test behavior on loss of direct transport. Do not hard-code production fallback policy or add an AirHand relay.
3. Validate actual live screen, keyboard and pointer input on two Macs over the same authenticated Tailcat UDP direct path, observe DERP data counters, and check disconnect/revocation and input release. Run CI after two-device acceptance. Do not change firewall or macOS security settings merely to manufacture a pass.
