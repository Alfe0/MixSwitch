# v4.2 changes since v4.1

- Flyout opacity: 249 -> 217 only in mixer mode; settings still 249.
- Header device bar: +10 px; 32px device symbol; larger title; cyan 28px gear.
- Mixer device symbol no longer cycles device icons on click; settings icon button unchanged.
- Recognized WhatsApp group now uses a dedicated embedded WhatsApp-style green logo.
- One-time v4.2 Windows Startup confirmation dialog for existing installs and fresh installs.
- Output device OSD width: 290 -> 244 px.
- Volume mouse wheel step: 2 -> 4; list scrolling unchanged.
- `audio.rs` unchanged from v4.1.

Note: The layered Win32 alpha setting changes opacity for the entire mixer window (not the backdrop alone). Building and runtime testing on Windows remain necessary.
