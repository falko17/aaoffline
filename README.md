# Ace Attorney Offline

Downloads cases from [Ace Attorney Online](https://aaonline.fr) to be playable offline.

## Features

- Backup cases in a way that makes them fully playable offline by downloading all referenced assets.
- Use parallel downloads to download case data quickly.
- Run `aaoffline-gui` to configure the download in a GUI instead of a terminal.
- Download multiple cases at once.
- Use the `-1` flag to compile the case into a single HTML file, without the need for a separate assets folder.
- Apply [userscripts](https://aaonline.fr/forum/viewtopic.php?t=13534) to get a better layout, keyboard controls, and a backlog.
- Choose a specific version of the Ace Attorney Online player (e.g., if a case only works with an older version).
- Automatically remove photobucket watermarks from downloaded assets.

## Usage

[Releases](https://github.com/falko17/aaoffline/releases) are provided for download, but you can also simply build the tool yourself (see *Building*).
There are two relevant files (each with a `.exe` extension for Windows, and no extension for Linux):
- `aaoffline`: The command-line tool. Intended to be run in a terminal. Explained in section *CLI*.
- `aaoffline-gui`: The graphical user interface. Can be run outside of a terminal. Explained in section *GUI*.

### GUI
![Screenshot of the GUI.](https://i.imgur.com/z1EibMH.png)

Simply add the URL[^1] (or ID) of the case on the top left. You can add multiple cases here to download them all at once, which is faster than downloading them one-by-one.
You can hover over each of the options to get an explanation. There's also a short overview of some options in the *CLI* section of this README.

When you're ready, just press "Download"—you'll see the download status at the top right, and any log messages below (this is also where errors will appear if there are any).

### CLI
Cases can be downloaded by just putting the trial ID as an argument to `aaoffline`:

```bash
aaoffline YOUR_ID_HERE
```

Or, even simpler, you can pass the URL[^1] to the case directly:

```bash
aaoffline "http://www.aaonline.fr/player.php?trial_id=YOUR_ID_HERE"
```

You can also pass more than one case at a time (separated by spaces) if you want to download multiple cases at once.

By default, the case will be put into a directory with the case title as its name. You can change this by just passing a different directory name as `-o some_directory`. If there are multiple cases, each case will be put into its own folder, again with the case title as its name, all under the directory chosen with `-o` (or the current directory if none was set).
The downloaded case can then be played by opening the `index.html` file in the output directory—all case assets are put in the `assets` directory, so if you want to move this downloaded case somewhere else, you'll need to move the `assets` along with it.
Alternatively, you can pass the `-1` flag to aaoffline, which causes the case to be compiled into a single (large) HTML file, with the assets encoded as data URLs instead of being put into separate files. (Warning: Browsers may not like HTML files very much that are multiple dozens of megabytes large. Your mileage may vary.)

There are some additional parameters you can set, such as `--concurrent-downloads` to choose a different number of parallel downloads to use[^2], `--player-version` to choose a specific commit of the player, or `--with-userscripts` to apply [userscripts](https://aaonline.fr/forum/viewtopic.php?t=13534).

To get an overview of available options, just run `aaoffline --help`.

## Building

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

### The asset downloads get stuck at some point and don't continue.

I still haven't figured out why/when this happens[^3], but what seems to help is lowering the `--concurrent-downloads`. If you still get this issue with concurrent downloads set to 1, please open an issue here.

### The blips sound weird in Firefox.

This is due to the HTML5 audio API being implemented differently in Firefox, refer to [#1](https://github.com/falko17/aaoffline/issues/1) for details.
As a workaround, use the `--disable-html5-audio` option (under advanced options in the GUI) with `aaoffline`, and then use a local HTTP server to serve the files—this way, the HTML5 audio API is not needed.
If you have Python installed, you can run `python3 -m http.server -d CASE_DIRECTORY` to run a simple web server, then you just need to access the URL it outputs.

[^1]: Both modern `aaonline.fr` and out-of-date `aceattorney.sparklin.org` URLs are supported.

[^2]: This is set to 5 by default, but a higher number can lead to significantly faster downloads. Don't overdo it, though, or some servers may block you.

[^3]: There is a timeout applied to HTTP reads/connects that's set to 30 seconds at most, so I'm not sure why the downloads can still get stuck like this.

### I get "The save you provided was not created on this trial" after finishing a case that is part of a sequence.

For case redirection within sequences to work correctly, `aaoffline` needs to know where each case is saved. This means that the whole sequence needs to be downloaded in a single run for `aaoffline` to set up jumps between cases, so please download all such cases at once (e.g., using `-s every` in the CLI, or *Sequence handling: every case* in the GUI).
