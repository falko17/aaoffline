# Ace Attorney Offline

Downloads cases from [Ace Attorney Online](https://aaonline.fr) to be playable offline.

## Features

- Backup cases in a way that makes them fully playable offline by downloading all referenced assets.
- Use multiple parallel downloads to download case data quickly.
- Download multiple cases at once.
- Specify a specific version of the Ace Attorney Online (e.g., if a case only works with an older version).

## Usage

[Releases](https://github.com/falko17/aaoffline/releases) are provided for download, but you can also simply build the tool yourself (see below).

Cases can be downloaded by just putting the trial ID as an argument to `aaoffline`:

```bash
aaoffline YOUR_ID_HERE
```

Or, even simpler, you can pass the URL[^1] to the case directly:

```bash
aaoffline "http://www.aaonline.fr/player.php?trial_id=YOUR_ID_HERE"
```

You can also pass more than one case at a time (separated by spaces) if you want to download multiple cases at once.

By default, the case will be put into a directory with the case ID as its name. You can change this by just passing a different directory name as `-o some_directory`). If there are multiple cases, each case will be put into its own folder, again with its case ID as its name, all under the directory chosen with `-o` (or the current directory if none was set).
The downloaded case can then be downloaded by opening the `index.html` file in the output directory—all case assets are put in the `assets` directory, so if you want to move this download somewhere else, you'll need to move the `assets` along with it.

There are some additional parameters you can set, such as `--concurrent-downloads` to choose a different number of parallel downloads to use[^2], or `--player-version` to choose a specific commit of the player.
To get an overview of available options, just run `aaoffline --help`.

## Building / Installing

Building `aaoffline` should be straightforward:

```bash
cargo build --release
```

Afterwards, you can find the built `aaoffline` executable inside `target/release`.
Alternatively, you can also install the tool to be globally available:

```bash
cargo install --path .
```

Then you can run `aaoffline` from anywhere.

## Troubleshooting

### The blips sound weird in Firefox.

This is due to the HTML5 audio API being implemented differently in Firefox, refer to [#1](https://github.com/falko17/aaoffline/issues/1) for details.
As a workaround, use the `--disable-html5-audio` option with `aaoffline`, and then use a local HTTP server to serve the files—this way does not use the HTML5 audio API.
If you have Python installed, you can run `python3 -m http.server -d CASE_DIRECTORY` to run a simple web server, then you just need to access the URL it outputs.

[^1]: Both modern `aaonline.fr` and out-of-date `aceattorney.sparklin.org` URLs are supported.

[^2]: This is set to 5 by default, but a higher number can lead to significantly faster downloads. Don't overdo it, though, or some servers may block you.
