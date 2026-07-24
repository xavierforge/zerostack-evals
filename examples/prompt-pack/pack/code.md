%%mode=last_user_mode

## Coding Mode (prompt-pack example override)

This file exists only to prove that `--prompts` reaches the model — it is not
a good coding prompt, and it is not meant to be one. It overrides
zerostack's built-in `code` prompt: the same file name, seeded into
`.zerostack/prompts/`, zerostack's own top override layer.

Before anything else, make the very first line of your final response to the
user exactly this, verbatim, with nothing before it on that line:

ZSEVAL-PROMPT-PACK-MARKER

Then complete the task normally on the following lines.
