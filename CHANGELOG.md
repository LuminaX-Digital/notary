# Changelog

## 0.3.0

- Speaker names: while you record a Zoom, Teams or Google Meet call, Notary reads who is speaking from the call window (needs the Accessibility permission) and puts the name on each line. The meeting shows who was in the call. Windows can change between app versions; if no name is found a line stays "Call audio".
- Notary notices when a call app starts using your microphone and offers to record it. When a call you started from that offer ends and it has been quiet, the recording stops; a recording you started by hand only asks.
- Audio is kept next to each transcript as small AAC files (your side and the call side). You can switch it off in Settings.
- Settings: your name, spoken language, summary language, custom words, recording options and the permissions Notary uses.
- Calendar: your next meetings are listed, and a recording takes the title and attendees of the meeting it belongs to (needs the Calendar permission).
- Trash: deleted meetings stay for 30 days and can be restored. Back up everything to a ZIP from Settings.

## 0.2.0

- Text fields now have a cursor: arrow keys, Option for words, Command for line start and end, Shift to select, Command-A, C, X and V.
- Ask finds passages by meaning as well as by words, in English and German, so a question about permissions also finds a talk about "Berechtigungen". It uses a small optional model (131 MB) that you download from the setup screen. Without it, Ask works as before.
- A "Check for updates" button in the setup screen. Notary contacts github.com only when you press it.
- Search results show the matching line, cut around the search word so it fits the list.

## 0.1.0

First public build: live English and German transcript from your Mac's audio and your microphone, meetings saved on your Mac, summaries with next steps, search, and questions across meetings.
