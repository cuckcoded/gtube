# GTube

A native GTK4 / libadwaita YouTube client written in Rust.

- **Your YouTube:** sign in and get the same home feed, subscriptions, history, playlists, Watch later and liked videos as on youtube.com. Like, dislike and subscribe from the app.
- **No ads:** videos play straight from the media streams, so video ads are never fetched. Promoted results, ad slots and banners are removed from every feed.
- **No Shorts:** they're removed from feeds, search, Up next and channel pages. A Shorts link still opens in the normal player, like any other video.
- **SponsorBlock:** segments are skipped automatically (with Undo) or marked in the seek bar, per category, and highlights get a marker and a "Jump to highlight" button. You can **submit segments** too: press `;` (or the shield button in the player) where one starts and again where it ends, check it in the dialog (Preview), pick the category and submit. Submissions go to an anonymous ID that GTube makes once and keeps in its settings.
- **Return YouTube Dislike:** estimated dislike counts and the like/dislike ratio bar.
- **Downloads:** video (up to 4K) or audio (mp3/m4a/opus/flac), whole playlists, with optional metadata/thumbnail/chapters, and SponsorBlock segments cut out.
- **Miniplayer:** keep watching in a corner while you browse. **Compact player:** shrink the whole window to just the player (Ctrl+Shift+M, the button next to fullscreen, or the menu). Resize it like any window and drag the video to move it; Esc or the same button brings the app back as it was. (Wayland doesn't let apps keep a window on top; most desktops have an "Always on top" item in the window menu, e.g. Alt+Space.) **Audio-only** mode streams just the audio.
- **Playlists and mixes:** a playlist plays as a queue next to the video, all of it (long ones load as they play), with loop and shuffle; Play all / Shuffle on playlist pages. Mixes (the endless "Mix – …" lists) show up in search and play as queues that keep going. Every video has Play next, Add to queue and Start mix in its menu, and the player gets previous / next buttons (Shift+P / Shift+N). `watch?v=…&list=…` links open with their playlist or mix.
- **Picks up where you left off:** GTube reports what you watch to YouTube the way youtube.com does, so your watch history shows how far you got. Videos you watched part of, here or on any other device, continue from where YouTube says you stopped (with "Start over" in the notification). Needs signing in; both can be turned off in Settings → Playback.
- **Pages** for watching (description, comments with replies, Up next, playlist queue), channels, playlists, search, and your own profile ("You"). Feed chips (All, Music, Gaming…) filter the feed; click the selected one again to go back to everything. The refresh button in the header (or Ctrl+R) reloads Home, Subscriptions, You and search results, keeping the selected chip.
- **YouTube Music mode:** switch with the YouTube | Music buttons at the top of the sidebar (or Ctrl+M). It works like music.youtube.com:
  - Home with mood chips and quick picks; Explore (new releases, charts, moods & genres); your Library (playlists, songs, albums, artists, subscriptions, history) and playlists in the sidebar.
  - Search with suggestions, a top result and filters (songs, videos, albums, artists, playlists, episodes…), plus artist, album and playlist pages.
  - A player bar with an endless queue: radios keep going, and autoplay continues with a mix when a list ends. Also shuffle, repeat (all / one), like / dislike, and play next / add to queue / start radio / go to album or artist / download on every song.
  - **Now playing:** the artwork or the music video (Song / Video switch), with Up next, Lyrics and Related.
  - **Immersive view** (F, or the button next to Now playing): the song across the whole screen, with big artwork or the video over a blurred wash of the art, a large visualizer and what's up next. The controls and pointer fade away while it plays; Esc leaves.
  - **Ambient color:** the window is tinted with the colors of the album art that is playing. **Spectrum visualizer:** the music's spectrum moves behind the player bar, in sync with what you hear. Both can be turned off in Settings → Playback → YouTube Music. The visualizer analyses the audio inside mpv (about 2% of one CPU core); GTube checks once per run that the installed FFmpeg has the filters it uses, and otherwise leaves it off.
  - Music plays in the background while you browse. Switch back to YouTube and it continues in the miniplayer.
  - `music.youtube.com` links open in Music mode. The mode you used last is remembered.

## Download

Builds for Linux, Windows and macOS come from GitHub Actions (`.github/workflows/build.yml`): every run keeps them as artifacts, and tagged versions (`v*`) are published as releases.

- **Linux:** `GTube-x86_64.AppImage`. Make it executable and run it. It needs glibc 2.41 or newer (Debian 13, Ubuntu 25.04, Fedora 42, Arch, or newer).
- **Windows:** `GTube-setup-x86_64.exe` installs it (for you, or for everyone); `GTube-windows-x86_64.zip` is the same thing as a folder you can run from anywhere (`GTube\bin\gtube.exe`).
- **macOS (Apple Silicon):** `GTube-macos-arm64.zip` holds `GTube.app`. It isn't notarized, so open it with right-click → Open the first time, or run `xattr -dr com.apple.quarantine GTube.app`.

The packages include GTK, libmpv, yt-dlp and deno. A yt-dlp installed on the system is used instead when there is one, since it's usually more up to date. Not included: ffmpeg (downloads need it to merge and convert; the Music visualizer needs it too and doesn't work on Windows) and the "Sign in with Google" window: sign in from your browser or with a cookies.txt file instead.

## Requirements

Runtime:

| Package | Why |
|---|---|
| `gtk4` ≥ 4.16, `libadwaita` ≥ 1.7 | UI |
| `mpv` (libmpv) | playback |
| `yt-dlp` (recent) | stream extraction, downloads |
| `deno` (or node) | yt-dlp needs a JS runtime to solve YouTube's challenges |
| `ffmpeg` | merging/converting downloads |
| `webkitgtk-6.0` | the "Sign in with Google" window (optional, see below) |

Arch: `pacman -S gtk4 libadwaita mpv yt-dlp deno ffmpeg webkitgtk-6.0`

Keep yt-dlp up to date: YouTube changes often, and most playback problems are fixed by a newer yt-dlp.

## Install / uninstall

```sh
./install.sh                     # build and install for your user (~/.local), adds it to the app menu
./install.sh uninstall           # remove it (keeps your settings, session and downloads)
./install.sh uninstall --purge   # also delete settings, session and caches
```

System-wide: `cargo build --release && sudo PREFIX=/usr/local ./install.sh` (and the same `PREFIX` to uninstall).

To just try it without installing: `cargo run --release`.

Without WebKit: `cargo build --release --no-default-features`. You can still sign in with a cookies.txt file or by importing from your browser.

`gtube <link or search terms>` opens a video, channel, playlist or search, including in an already running window. `gtube --help` lists the options.

## Signing in

Open **Sign in** in the sidebar or account menu. There are three ways:

1. **Sign in with Google:** a normal Google login page in an embedded browser window.
2. **From browser:** copies your existing youtube.com session out of Firefox, Chrome, Chromium, Brave, Edge, … (uses `yt-dlp --cookies-from-browser`).
3. **cookies.txt:** import a Netscape cookies file exported by a browser extension.

Only youtube.com cookies are kept. They're stored in `~/.config/gtube/cookies.txt` (mode 600), and signing out deletes them.

GTube talks to YouTube the way the website does, so your recommendations, history and subscriptions stay in sync. A third-party client is still against YouTube's terms of service; it's very unlikely to cause problems, but use it knowing that.

## How ad blocking works

YouTube serves ads through the web player. GTube doesn't use that player. yt-dlp resolves the actual audio/video streams and mpv plays them, so there's nothing to block. In feeds, every ad and promoted renderer is dropped while parsing. Settings → General shows how many ads and Shorts were removed this session.

When YouTube attaches a pre-roll ad, its servers hold the video back for the ad's length (usually ~5 s). You don't see the ad, but you do see a short **"Starting in N s"** countdown. Waiting this out is enforced on YouTube's side and can't be avoided.

## Keyboard shortcuts

| Key | Action |
|---|---|
| Space / K | play / pause |
| J / L | back / forward 10 s |
| ← / → | back / forward 5 s (player view) |
| ↑ / ↓ | volume (fullscreen) |
| F | fullscreen (Music: immersive view) |
| Esc | leave fullscreen / immersive view / compact player |
| Ctrl+Shift+M | compact player |
| ; | mark a SponsorBlock segment's start, then its end |
| M | mute |
| I | miniplayer / back to the video |
| A | audio only (Music: Song / Video) |
| Shift+N / Shift+P | next / previous (queue, playlist, mix or song) |
| + / − | volume (Music) |
| Ctrl+M | switch between YouTube and YouTube Music |
| / or Ctrl+L | search |
| Ctrl+R | refresh |
| Alt+← | back |
| Alt+1 … Alt+4 | Home, Subscriptions, You, Downloads |
| Ctrl+, | settings |
| Ctrl+Q | quit |

## Files

- `~/.config/gtube/settings.json`: settings
- `~/.config/gtube/cookies.txt`: your session (if signed in)
- `~/.local/share/gtube/webkit/`: the sign-in window's browser data
- `~/.cache/gtube/gtube.log`: log of the current run (`gtube.log.1`: the previous one). Errors, warnings and crashes end up here; URLs are stripped of their query strings and cookies are never logged.
- Downloads go to `~/Videos/GTube` by default (changeable in Settings → Downloads)

## Development

```sh
cargo test
# Parser checks against saved InnerTube responses (search.json, next.json,
# comments.json, channel.json, playlist.json, …):
GTUBE_FIXTURES=/path/to/responses cargo test fixtures -- --nocapture
# The same for YouTube Music responses (home.json, search_all.json, artist.json,
# album.json, next_radio.json, lyrics.json, …):
GTUBE_MUSIC_FIXTURES=/path/to/responses cargo test music_fixtures -- --nocapture
```

Layout: `src/api` (InnerTube client, parsers, SponsorBlock/RYD; `music.rs` is YouTube Music), `src/extract.rs` (yt-dlp), `src/mpv.rs` (libmpv FFI, OpenGL render API), `src/ui` (pages and widgets; `ui/music/` is Music mode).

Packaging lives in `packaging/`: `linux/appimage.sh`, `macos/bundle.sh`, `windows/package.sh` and `windows/gtube.iss` (Inno Setup), plus `fetch-tools.sh` for the bundled yt-dlp and deno. Each script's header lists what it needs; the workflow runs them.

`GTUBE_MPV_AO` overrides mpv's audio output (e.g. `null` for headless tests). `GTUBE_SPONSORBLOCK_API` points SponsorBlock lookups and submissions at another server (e.g. a local one, to try submitting without touching the real database). `GTUBE_STATS_DRY_RUN=1` writes the watch-history reports to the log instead of sending them (also when signed out), see `src/api/stats.rs`. The visualizer's levels pass through a FIFO in `$XDG_RUNTIME_DIR` (`gtube-spectrum-<pid>`, removed on exit); see `src/ui/music/spectrum.rs`.

The window's actions are exported over D-Bus, which is handy for scripting:

```sh
gdbus call --session --dest io.github.gtube --object-path /io/github/gtube/window/1 \
  --method org.gtk.Actions.Activate open "[<'https://youtu.be/jNQXAC9IVRw'>]" {}
# Music mode, its Now playing view:
gdbus call --session --dest io.github.gtube --object-path /io/github/gtube/window/1 \
  --method org.gtk.Actions.Activate mode "[<'music'>]" {}
gdbus call --session --dest io.github.gtube --object-path /io/github/gtube/window/1 \
  --method org.gtk.Actions.Activate now-playing "[]" {}
```

## License

GPL-3.0-or-later
