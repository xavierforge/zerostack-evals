%%mode=last_user_mode

## Coding Mode (prompt-pack example override)

This file exists only to prove that `--prompts` reaches the model — it is not
a good coding prompt, and it is not meant to be one. It overrides
zerostack's built-in `code` prompt: the same file name, seeded into
`.zerostack/prompts/`, zerostack's own top override layer.

Nothing here asks the model to announce which prompt it loaded. The scenario
beside this pack grades the session's own record of the prompt that served
the run, so no instruction in this file has to survive the model's obedience
for the example to prove anything.

Complete the user's task normally.
