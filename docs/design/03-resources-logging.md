# 03 — Resources, leak detection, and logging

How vaht guarantees it releases everything it acquires, how a failure to do so is detected when
it doesn't, and how it logs. This is a draft for discussion.

---

## 1. What can leak in a compositor

| Kind | Examples | Released by |
|---|---|---|
| Heap objects | Windows, panels, menus, caches, per-client state | Owner dropped |
| File descriptors | Client sockets, dmabufs, syncobjs, DRM and input devices (via libseat), inotify, pidfds, IPC connections, D-Bus, log files | `OwnedFd` dropped |
| GPU memory | Textures (panel buffers, wallpaper, icons), framebuffers, swapchains | Smithay texture handles dropped |
| Event sources | Timers (auto-raise, key repeat, tooltip, debounce), fd sources | calloop `RegistrationToken` removed |
| Processes | Children vaht started, the menu helper, Xwayland | Reaped (pidfd) |
| Protocol state | Objects a disconnected client left behind | Smithay client cleanup; vaht's `UserData` dropped with it |
| Unbounded growth | Glyph cache, icon cache, history lists, IPC send buffers | **Bounds**, not ownership |

The last row matters: a cache that only ever grows is a leak no ownership
rule can catch.

---

## 2. Structural guarantees (bugs made impossible, not detected)

1. **One owner per resource; ids elsewhere** (01 §1.1). Nothing holds a
   reference into another subsystem's objects, so nothing can keep an object
   alive by accident or point at a freed one.
2. **`unsafe_code = "forbid"`** in every vaht crate (R-X2). File descriptors
   are `OwnedFd` / `BorrowedFd`; no raw-fd juggling is possible in vaht's code.
3. **No inherited descriptors.**
   - Rust's standard library and Smithay open every descriptor close-on-exec,
     so children never inherit the compositor's descriptors.
   - The `Launcher` is the only place that spawns. It passes an explicit
     environment and standard streams and nothing else. A test asserts that a
     child's `/proc/<pid>/fd` holds only 0, 1 and 2.
4. **Every event-source registration is an RAII guard.**
   - **`EventSource`** holds the loop handle and token, and removes the source
     in `Drop`.
   - **`Timer`** wraps one for timeouts; dropping it cancels the timeout.
   - A window that owns an auto-raise `Timer` therefore cannot leave a timer
     firing after it closes. fluxland had to remove each one by hand.
   - The raw `insert_source` is called only inside `EventSource::new`, which a
     clippy `disallowed_methods` entry enforces.
5. **Processes are reaped by event, never by `waitpid(-1)`.**
   - Each child gets a pidfd registered as a calloop source; it is reaped when
     it exits and logged with its exit status.
   - Profile hooks and dialogs have a configurable timeout, after which they
     are killed and logged.
   - There are no double forks: fluxland needed them only because of the
     SIGCHLD race with wlroots, which pidfds don't have.
6. **Every cache has a bound and an eviction policy, in its type.**
   - `BoundedCache<K, V>` (in `vaht-ui`) has an entry limit, a byte limit and
     least-recently-used eviction. Glyph cache, icon cache, scaled wallpapers
     and text layouts all use it.
   - Per-output caches are dropped with their `Output`.
   - The capacities are named constants explaining "why this value" (K1).
7. **No reference-counted cycles.** `Rc<RefCell<…>>` graphs are not used
   (R-O4). The only shared ownership is what Smithay itself uses for surfaces
   (`Arc`), and any back-link vaht keeps is a `Weak` or an id.
8. **Client resources are scoped to the client.** Per-client data lives in
   Smithay's `ClientData` / `UserData`, and is dropped with the client. IPC
   connections are dropped on the first error or EOF. Their send buffers are
   bounded: a subscriber that stops reading is disconnected rather than buffered
   without limit.

---

## 3. Detection

### 3.1 `ResourceLedger`: live counts for every resource kind

- `ResourceKind` is an enum: `Window`, `Panel`, `PanelBuffer`, `Texture`,
  `Timer`, `EventSource`, `ChildProcess`, `IpcConnection`, `Output`,
  `MenuSession` and so on.
- Each owning type embeds a `Tracked<K>` field. Constructing the type
  increments a relaxed atomic counter for its kind, and dropping it decrements
  the counter. The cost is one atomic increment per object creation, which is
  negligible, so it is **always on**, in release builds too. That means
  `vaht msg debug resources` works on a real session, not only in a debug
  build.
- `vaht msg debug resources` reports:
  - live counts per kind, with their peak values;
  - open file descriptors grouped by what they point to (socket, `anon_inode`
    type such as dmabuf, syncfile or eventfd, DRM, pipe, file), read from
    `/proc/self/fd`;
  - resident memory;
  - each `BoundedCache`'s entries and bytes against its limit.
- **Leaks show up as numbers that only go up.** Run the same workload twice (open
  and close 50 windows) and compare.

### 3.2 Invariant checks (debug builds)

After every dispatched event batch, `Compositor::check_invariants()` asserts
that:
- every `WindowId` in `Stack`, `Focus`, the taskbar window lists, marks and
  grabs exists in `Windows`;
- the `Window` count equals the number of mapped toplevels that Smithay reports;
- the panel count equals frames × 2 + taskbars + open menus + tooltip;
- no `EventSource` is registered for an object that no longer exists.

A failure is a bug (E1), so it panics with the invariant's name. Release
builds skip these checks.

### 3.3 Tests that would catch a leak

1. **The headless test compositor.** A `Backend::Headless` variant renders
   with Smithay's pixman renderer into memory. Integration tests start vaht in
   process and connect real Wayland test clients (`wayland-client`), so they
   need no GPU and no session.
2. **Churn tests.** Open and close 500 windows, open and close menus, hotplug
   outputs, and connect and disconnect IPC clients. After each, the ledger and
   the fd count must return to their baseline exactly. These run in
   `./checks`.
3. **Child descriptor test** (§2.3).
4. **Optional deeper runs**, not in `./checks`:
   - `valgrind --tool=massif` or `heaptrack` on a scripted session;
   - `dhat` behind a `heap-profiling` cargo feature;
   - a nightly CI job with `-Zsanitizer=leak`.

---

## 4. Logging

### 4.1 Mechanism

- **`tracing` throughout, with structured fields** (R-A1), for example
  `debug!(window = %id, app_id, "mapped")`. Smithay already logs through
  `tracing`, so its messages land in the same places.
- **Disabled levels cost almost nothing.** `tracing` caches each call site's
  enabled state, so an unused `trace!` costs a load and a branch. Detail can
  live in the code permanently and be switched on when needed.
- **Every write is off the compositor thread.** All file output goes through
  `tracing-appender`'s non-blocking writer, a bounded channel to a writer
  thread. The compositor never waits on disk.
  - If the disk can't keep up, lines are **dropped and counted** rather than
    stalling a frame.
  - The count is logged once the backlog clears, and appears in
    `vaht msg debug resources`.

### 4.2 Choosing what is logged

| Level | Contains |
|---|---|
| `error` | Failures that lost something: a config could not be applied, an output failed a modeset, a helper crashed |
| `warn` | Recoverable surprises: a client protocol error (with its pid and app id), a refused activation, a dropped log backlog |
| `info` (default) | Lifecycle, rarely: start and stop, output added or removed, profile applied, config reloaded, children exiting non-zero |
| `debug` | Per-window and per-action events: map, unmap, focus changes, commands executed, grabs |
| `trace` | Per-frame and per-input-event detail: scene composition, damage, input routing |

- **Filters use the `RUST_LOG` syntax**, for example
  `info,vaht::input=trace,smithay::backend::drm=debug`. They come from
  `--log` or `VAHT_LOG` at startup.
- **They can change while vaht runs:** `vaht msg log-filter "vaht::focus=debug"`,
  through `tracing-subscriber`'s reload layer. Tracking down a focus bug no
  longer needs a restart, and `vaht msg log-filter reset` returns to the
  startup filter.
- **Privacy:** window titles and command lines appear only at `debug` and
  below, since titles often contain document names and other personal text.

### 4.3 Where it goes, and rotation

| Sink | Default | Notes |
|---|---|---|
| **File**: `$XDG_STATE_HOME/vaht/vaht.log` (normally `~/.local/state/vaht/`) | on | Size-based rotation with `file-rotate` (well-used, 6M+ downloads). It rotates to `vaht.log.1`, `.2`, … at **10 MB per file, keeping 5**, so 50 MB at most. **Each start also rotates**, so `vaht.log` is always the current session and `vaht.log.1` the previous one, which is the one you want after a crash. Both numbers are settings |
| **stderr** | on when stderr is a terminal | Started from a TTY, this is the VT behind the compositor: useful if vaht dies before its file sink opens |
| **journald** | on when journald's socket exists | `tracing-journald` writes to `/run/systemd/journal/socket`. It works whether or not vaht itself is started as a systemd unit, and journald applies its own system-wide size limits. Structured fields become journal fields, so `journalctl -t vaht WINDOW=12` works. Read it with `journalctl -t vaht` |

**Why size-based rotation in-process, not logrotate.** logrotate is
system-level, runs on a schedule, and needs `copytruncate` for a process that
keeps its log open, which loses lines. journald is the canonical answer on
systemd systems and is offered as an option. Rotating inside vaht by size
works the same everywhere and needs nothing set up. `tracing-appender`'s own
rolling writer only rotates by time, which doesn't bound disk use.

### 4.4 Crashes and diagnostics

- **Panic hook.** It logs the panic message, location and backtrace to every
  sink, flushes the non-blocking writer (it keeps the `WorkerGuard`), restores
  the VT, and exits non-zero.
- **`vaht msg debug frames`** reports per-output frame statistics: render
  time, missed vblanks, direct scanout hits and misses, and whether VRR is
  active. This is the first stop when a game or video stutters.
- **`tracy` cargo feature (off by default).** `tracing-tracy` streams spans
  for scene composition, panel painting, rendering and input handling to the
  Tracy profiler. It replaces fluxland's compile-time `perf.c` probes and adds
  no cost when not compiled in.

---

## 5. Resolved questions

1. **File sink:** 10 MB × 5 files, plus a rotation at each start.
2. **journald:** on by default whenever it is present, alongside the file.
