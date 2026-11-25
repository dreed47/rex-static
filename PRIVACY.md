# Privacy policy

Rex Static is a desktop app that runs on your own computer. This policy covers
its optional Gmail and Google Calendar features.

## What Rex Static accesses

When you turn on `google = yes` and sign in, Rex Static asks Google for:

- **Gmail read** (`gmail.readonly`): to search and read your mail when you ask.
- **Gmail compose** (`gmail.compose`): to save drafts. Rex Static never sends mail.
- **Calendar events** (`calendar.events`): to list your events, and to add,
  move or delete one after you say yes.

## Where your data goes

- Rex Static talks to Google directly from your computer. There is no Rex
  Static server, and the author never receives your data.
- To answer you, the mail or calendar text that Rex Static reads is sent to the
  Anthropic Claude API under your own API key, as part of the conversation.
  Anthropic's own privacy policy covers that.
- Your Google sign-in token is stored in your system keyring
  (`secret-tool`, service `rex-static`). It is not written to a file.

## Removing access

- Remove the app at <https://myaccount.google.com/permissions>.
- Delete the stored token:
  `secret-tool clear service rex-static key google-refresh`

## Contact

Open an issue at <https://github.com/dreed47/rex-static/issues>.
