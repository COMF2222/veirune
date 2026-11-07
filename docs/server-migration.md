# Verified service moves

Desktop 0.4.31 reads `https://fastcloud.comf.workers.dev/service.json` natively.
Only a valid P-256/SHA-256 announcement under the pinned owner key can replace
an owned service endpoint. Public discovery gets no OAuth/cookies. The signed
envelope is bounded, persisted and protected against serial rollback/conflicts;
custom servers are left alone. A broken/offline address book leaves the last
verified route in use. Healthy playback does not wait for discovery requests.

The broker address is resolved when requests are built; remote token exchange
and refresh also use the verified route. The service checks announcements on a
45-second cadence and rebuilds a media source after an address change, keeping
the current queue/position. Paused queues discard stale capabilities and stay
paused. User grants remain in the same OS keyring entries. The historical
77.223 → 80.47 recovery still requires reauthentication because that earlier
server's database was lost; it is not reused for future restored migrations.

The owner must restore the original SQLite database and `.env`, preserving the
SoundCloud Client ID, secret and redirect URI. Recreating the SoundCloud API app
cannot preserve its existing user grants. Source scripts restore a new VPS,
verify all table contents, certificates and readiness, then prepare a signed
website update. Cached audio and ephemeral tickets are rebuilt. Client migration
support must be installed once before the old VPS disappears; older released
binaries cannot learn this protocol retroactively.

See the sibling backend's `MIGRATION.md` for the exact copy/restore/cutover steps.
The current host was not stopped or replaced during verification. A real second
VPS was not supplied: cross-host Docker/ACME rollout and live device cutover are
not claimed tested. macOS playback remains a Mac device check.
