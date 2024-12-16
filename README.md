# Ace Attorney Offline

Downloads cases from [Ace Attorney Online](https://aaonline.fr) to be playable offline.

> [!WARNING]
> This is in an early state and may not work correctly. Currently, I'm aware of the following problem:
>
> 1. Psyche locks won't work correctly.
>
> Additionally, the code is still a bit messy and largely undocumented.

## Usage

[Releases](https://github.com/falko17/aaoffline/releases) are provided for download, but you can also simply build the tool yourself (see below).

Cases can be downloaded by just putting the trial ID as an argument to `aaoffline`:

```bash
aaoffline YOUR_ID_HERE
```

Or, even simpler, you can pass the URL to the case directly:

```bash
aaoffline "http://www.aaonline.fr/player.php?trial_id=YOUR_ID_HERE"
```

By default, the case will be put into a directory with the case ID as its name (you can change this by just passing a different directory name as another argument).
The downloaded case can then be downloaded by opening the `index.html` file in the output directory.

There are some additional parameters you can set, such as `--concurrent-downloads` to choose a different number of parallel downloads to use[^1], or `--player-version` to choose a specific commit of the player (e.g., if a case only worked on an older version).
To get an overview of available options, just run `aaoffline --help`.

[^1]: This is set to 5 by default, but a higher number can lead to significantly faster downloads. Don't overdo it, though, or some servers may block you.

## Building

Building `aaoffline` should be straightforward:

```bash
cargo build --release
```

Afterwards, you can find the built `aaoffline` executable inside `target/release`.

## Troubleshooting

### The blips sound weird in Firefox.

This is due to the HTML5 audio API being implemented differently in Firefox, refer to [#1](https://github.com/falko17/aaoffline/issues/1) for details.
As a workaround, use the `--disable-html5-audio` option with `aaoffline`, and then use a local HTTP server to serve the files—this way does not use the HTML5 audio API.
If you have Python installed, you can run `python3 -m http.server -d CASE_DIRECTORY` to run a simple web server, then you just need to access the URL it outputs.
