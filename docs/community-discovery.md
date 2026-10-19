# Community discovery (0.4.22 / Mobile 0.1.9)

The home screen shows popular songs and a shared Editors’ picks collection.
`show_community` is an account preference, enabled by default and synchronized
between desktop and mobile. Hiding it does not disable listening statistics.

## Popular music

The existing listening history supplies the ranking immediately. For each track,
the backend groups all devices and days by account before counting listeners.
An account qualifies after at least 30 seconds and one counted play in the
selected period. Repeating a song never gives that account extra votes.
Only approved accounts count, and a track needs at least two distinct listeners.
Each period returns up to five public tracks.

Days follow the existing statistics storage timezone, UTC. Today and yesterday
are calendar days; Week is today plus the previous six calendar days. Track ID
breaks equal listener counts deterministically. Missing or private metadata is
omitted instead of exposing private listening history or stream URLs.

## Editors

Only the configured backend owner can see User management and appoint editors.
An editor does not receive any user-management, ban or server-admin permissions.
The owner is an editor automatically. Editors use the star beside the track in
the bottom/full player (or the mobile track menu). Each editor may choose ten
tracks; the shared collection holds up to thirty distinct tracks. Several editors
can recommend the same track, which appears once. Editors can withdraw their own
recommendations; the owner can remove a track from the shared collection.
Revoking an editor removes their recommendations. Blocked accounts do not vote.

Deploy the backend first, then desktop 0.4.22 and mobile 0.1.9. No environment
variables or manual database migration are required. Startup/preflight creates
the new tables; normal whole-database backups include roles and recommendations.
Preference protocol 3 adds the visibility setting; protocol 1/2 clients retain
their previous field sets and cannot erase this new preference.

## Desktop window freeze investigation

A live installed Windows process was marked unresponsive while about 3.3 GiB of
physical memory remained free. Its process tree occupied roughly 325 MiB of
working set. Windows denied read access to its wait chains, so the exact blocked
stack was not captured. The running process was left untouched.

Source review found a possible settings → audio preferences → playback state →
settings lock cycle. Player polling now reads settings before locking playback;
transition preparation copies audio preferences before accessing playback.
Cloud sync still serializes preference changes to keep the saved settings and
audio settings consistent. Player polling also
runs outside the native window event loop. A bounded concurrency
regression checks that transition preparation waiting for playback does not
block a preference update. This prevents the identified cycle; it does not prove
that every cause of an unresponsive window has been resolved.

## Verification limits

Backend tests exercise ranking, periods, role separation, metadata privacy,
editor limits, revocation and old preference clients. Desktop Rust and frontend
checks run locally. Mobile is built as a signed release APK with the existing
certificate; browser checks cover community controls, editor/owner/listener
roles, light/dark themes and English/Russian layouts. Actual phone and macOS
runtime behaviour still requires device testing. No production deploy is implied
by these local checks.
