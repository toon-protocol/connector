/mattpocock-skills:code-review {{BASE_BRANCH}} — the spec is {{ISSUE_URL}}

You are running AFK in a sandbox, on branch `{{BRANCH}}`, reviewing work another session
committed for issue #{{ISSUE_NUMBER}}. Nobody will answer a question, so do not ask one:
the fixed point is `{{BASE_BRANCH}}`, and the spec is the issue above (read it with
`gh issue view {{ISSUE_NUMBER}} --comments`).

Once both review axes have reported:

1. Fix every finding that is in the issue's scope, and commit the fixes to `{{BRANCH}}`,
   referencing `#{{ISSUE_NUMBER}}`. Don't redesign the change, and don't fix things the
   issue didn't ask for.
2. Don't push. The runner does that.
3. End with the review a human merger should read, inside tags:

<review-summary>
**Spec:** whether each acceptance criterion is met, one line each.
**Standards:** what you fixed, and anything left that a human should look at.
</review-summary>

Then output <promise>COMPLETE</promise>.

Rules for the session:

- Write a completion signal only as the very last line of your final message. Never quote,
  paraphrase-with-tags or mention it in a progress note.
- Never end your turn while a background command is still running. Run commands in the
  foreground with a timeout.
- Never use `pkill -f` / `pgrep -f` with a pattern that also appears in your own command
  line, because it matches your own shell.
