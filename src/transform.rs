//! Contains methods and models related to the AAO player transformation.

/// Contains methods and models related to detecting and transforming PHP blocks.
pub(crate) mod php {
    use std::ops::Range;
    use std::sync::LazyLock;

    use anyhow::Result;
    use log::{error, trace, warn};
    use regex::Regex;

    use crate::constants::{re, UPDATE_MESSAGE};
    use crate::data::case::Case;
    use crate::data::player::Player;

    /// A matched <?php ... ?> block within the player.
    #[derive(Clone, Debug)]
    struct FoundPhpBlock {
        start: usize,
        end: usize,
        replaced: Option<String>,
    }

    impl FoundPhpBlock {
        fn new(start: usize, end: usize) -> FoundPhpBlock {
            FoundPhpBlock {
                start,
                end,
                replaced: None,
            }
        }
        fn new_unexpected(start: usize, end: usize) -> FoundPhpBlock {
            FoundPhpBlock {
                start,
                end,
                replaced: Some(String::new()),
            }
        }
    }

    /// An expected <?php ... ?> block within the player.
    #[derive(Debug)]
    struct ExpectedPhpBlock {
        /// A human-readable ID for this block to uniquely identify it.
        id: &'static str,
        range: Option<Range<usize>>,
        /// A regex to detect this PHP block.
        detector: LazyLock<Regex>,
        /// A function with which the contents of this PHP block can be transformed.
        replacer: Option<fn(&Player, &Case) -> Result<String>>,
    }

    impl ExpectedPhpBlock {
        fn expect_match(&self, other: &FoundPhpBlock) {
            if let Some(Range { start, end }) = self.range {
                if (other.start, other.end) != (start, end) {
                    warn!(
                    "Expected PHP block {} to be at character range ({start}–{end}), but was at ({}–{}). {UPDATE_MESSAGE}",
                    self.id, other.start, other.end
                );
                }
            }
        }

        fn replace(&self, player_scripts: &Player, case: &Case) -> Result<String> {
            if let Some(replacer) = self.replacer {
                replacer(player_scripts, case)
            } else {
                Ok(String::new())
            }
        }

        fn matches(&self, text: &str) -> bool {
            trace!(
                "Matching PHP block {} with {} to {text}...",
                self.id,
                self.detector.to_string()
            );
            self.detector.is_match(text)
        }

        const fn new(
            id: &'static str,
            start: usize,
            end: usize,
            detector: LazyLock<Regex>,
            replacer: Option<fn(&Player, &Case) -> Result<String>>,
        ) -> ExpectedPhpBlock {
            ExpectedPhpBlock {
                id,
                range: Some(start..end),
                detector,
                replacer,
            }
        }

        const fn new_rangeless(
            id: &'static str,
            detector: LazyLock<Regex>,
            replacer: Option<fn(&Player, &Case) -> Result<String>>,
        ) -> ExpectedPhpBlock {
            ExpectedPhpBlock {
                id,
                range: None,
                detector,
                replacer,
            }
        }
    }

    fn transform_blocks(
        player: &Player,
        case: &Case,
        source: &mut String,
        blocks: &[ExpectedPhpBlock],
    ) -> Result<()> {
        let mut visited: Vec<usize> = Vec::with_capacity(blocks.len());
        let mut replacements: Vec<FoundPhpBlock> = Vec::new();
        for block_match in re::PHP_REGEX.captures_iter(source) {
            let text = block_match
                .get(1)
                .expect("No captured content in PHP block")
                .as_str()
                .to_string();
            let whole_match = block_match.get(0).unwrap();
            let start = whole_match.start();
            let end = whole_match.end();
            trace!("PHP block: {text}");

            let visited_until_now = visited.clone();
            let copied_text = text.clone();
            let result: Vec<_> = blocks
                .iter()
                .enumerate()
                .filter(move |x| !visited_until_now.contains(&x.0) && x.1.matches(&copied_text))
                .collect();

            let replacement = if result.is_empty() {
                warn!("Unexpected PHP block at ({start}–{end}). Removing from HTML.",);
                FoundPhpBlock::new_unexpected(start, end)
            } else if result.len() > 1 {
                error!(
                    "Invalid ({}) matches for PHP block at ({start}–{end}). {UPDATE_MESSAGE}",
                    result.len(),
                );
                std::process::exit(exitcode::SOFTWARE);
            } else {
                let result = result[0];
                let mut block = FoundPhpBlock::new(start, end);
                result.1.expect_match(&block);
                block.replaced = Some(result.1.replace(player, case)?);

                // Mark block as visited.
                visited.push(result.0);

                block
            };

            replacements.push(replacement);
        }

        // Sort replacements by reverse order of position so we can safely replace them.
        replacements.sort_by(|a, b| b.start.cmp(&a.start));

        for replacement in replacements {
            let start = replacement.start;
            let end = replacement.end;
            let replaced = if let Some(replaced) = replacement.replaced {
                replaced
            } else {
                warn!(
                    "Unhandled PHP block at ({}–{}). Removing from HTML.",
                    start, end
                );
                String::new()
            };
            source.replace_range(start..end, &replaced);
        }

        Ok(())
    }

    pub(crate) fn transform_trial_blocks(
        player: &Player,
        case: &Case,
        source: &mut String,
    ) -> Result<()> {
        static EXPECTED_TRIAL_BLOCKS: [ExpectedPhpBlock; 2] = [
            ExpectedPhpBlock::new_rangeless(
                "common_render",
                LazyLock::new(|| Regex::new(r"include\('common_render\.php'\);").unwrap()),
                None,
            ),
            ExpectedPhpBlock::new_rangeless(
                "trial_data",
                LazyLock::new(|| Regex::new(r"var trial_information;").unwrap()),
                Some(|_, case| case.serialize_to_js()),
            ),
        ];

        transform_blocks(player, case, source, &EXPECTED_TRIAL_BLOCKS)
    }

    pub(crate) fn transform_player_blocks(player: &mut Player, case: &Case) -> Result<()> {
        static EXPECTED_PLAYER_BLOCKS: [ExpectedPhpBlock; 5] = [
            ExpectedPhpBlock::new(
                "common_render",
                1,
                40,
                LazyLock::new(|| Regex::new(r"include\('common_render\.php'\);").unwrap()),
                None,
            ),
            ExpectedPhpBlock::new(
                "language",
                224,
                272,
                LazyLock::new(|| Regex::new(r"echo language_backend\(.*\)").unwrap()),
                Some(|p, _| Ok(p.args.language.clone())),
            ),
            ExpectedPhpBlock::new(
                "script",
                276,
                396,
                LazyLock::new(|| Regex::new(r"include\('bridge\.js\.php'\);").unwrap()),
                Some(|p, _| {
                    Ok(p.scripts
                        .as_ref()
                        .unwrap()
                        .scripts
                        .as_ref()
                        .unwrap()
                        .clone())
                }),
            ),
            ExpectedPhpBlock::new(
                "title",
                417,
                530,
                LazyLock::new(|| {
                    Regex::new(r"echo 'Ace Attorney Online - Trial Player \(Loading\)';").unwrap()
                }),
                Some(|_, case| Ok(case.trial_information.title.clone())),
            ),
            ExpectedPhpBlock::new_rangeless(
                "heading",
                LazyLock::new(|| Regex::new(r"echo 'Loading trial \.\.\.';").unwrap()),
                Some(|_, case| Ok(case.trial_information.title.clone())),
            ),
        ];

        let mut playertext = player.player.as_mut().unwrap().clone();
        transform_blocks(player, case, &mut playertext, &EXPECTED_PLAYER_BLOCKS)?;
        player.player = Some(playertext);
        Ok(())
    }
}
