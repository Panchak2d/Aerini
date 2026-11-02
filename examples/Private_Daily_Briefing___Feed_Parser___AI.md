# Example — Private Daily Briefing (Feed Parser → AI)

Reads an RSS or Atom feed every morning, takes the newest item you have not seen yet, has a local model summarize it in two sentences, and appends the result to a text file and shows a notification. A day with nothing new adds a one-line log entry and no notification, and a failed fetch or summary raises an alert. No Code node, no cloud AI, no API key.

## Nodes used

| Node | Purpose |
|------|---------|
| Schedule (Every Morning) | Cron `0 8 * * *` |
| HTTP Request (Fetch Feed) | `GET` the feed. The URL in the file is a placeholder: replace it |
| Feed Parser (Newest Item) | Text `{{Fetch Feed.output.body}}`, Base URL the feed's address, Only New on, On First Run `baseline`, Max Items `1` |
| If / Condition (Anything New) | Condition `{{Newest Item.output.count}} > 0`; True goes to Summarize, False to Log Nothing New |
| AI Prompt (Summarize) | Provider `local`, Base URL `http://localhost:11434/v1`, Model `llama3` (edit to a model you have pulled), a System message that asks for two plain sentences with no preamble, Temperature `0.3`, Timeout `600` |
| File (Append to Briefing) | Operation `append`, Path `daily-briefing.txt` |
| Desktop Notification (Notify Me) | Shows the title and the summary |
| File (Log Nothing New) | On False: appends `<date>: nothing new` to the same file |
| Output (Nothing New, Briefing Done) | Ends each branch the same way, so a run never stops mid-graph |
| Desktop Notification (Alert Fetch Failed) | On Fetch Feed's Error port: says the feed could not be fetched |
| File (Save Unsummarized) and Desktop Notification (Alert Summary Failed) | On Summarize's Error port: saves the title and link without a summary, then alerts you |

## How it behaves

- **First run is silent.** With On First Run `baseline`, Feed Parser remembers every item currently in the feed and returns none, so `count` is `0`, the If sends nothing down True, and the run ends. From the next run, only items that appeared since then come through.
- **One item per run.** Max Items `1` returns the newest unseen item. If two new items appear in one day, the second is returned on the next run, because only returned items are marked as seen.
- **Quiet days stay quiet.** No new item means `count` is `0`, so the model is not called and no notification appears. The False port of Anything New appends one line, `<date>: nothing new`, to the briefing file, so the file shows the workflow ran. Without a node on that port, the run just ends, with a log entry saying the branch was dropped.
- **Delivered at most once.** Feed Parser marks an item as seen when it runs, so a failed summary does not bring the item back. If the model fails or does not answer within its 600 second timeout, Save Unsummarized writes the title and link to the briefing file without a summary and Alert Summary Failed notifies you.
- **Failures are not silent.** A failed fetch (after 3 attempts, 2 seconds apart) shows Alert Fetch Failed. A feed that fetches but is not a feed, such as an HTML error page, fails Feed Parser, which has no Error port: the run is recorded as failed in the **History** tab without an alert.

## How to build it

If you import `Private_Daily_Briefing___Feed_Parser___AI.aerini` you can skip to [Edit before running](#edit-before-running). To build it by hand:

1. Add a **Schedule** node named **Every Morning**: Mode `cron`, Cron Expression `0 8 * * *`.
2. Add an **HTTP Request** named **Fetch Feed**: Method `GET`, URL your feed.
3. Add a **Feed Parser** named **Newest Item**.
   - **Text**: `{{Fetch Feed.output.body}}`
   - **Base URL**: the feed's address
   - **Only New** on, **On First Run** `baseline`, **Max Items** `1`
4. Add an **If / Condition** named **Anything New**: Condition `{{Newest Item.output.count}} > 0`.
5. On **True**, add an **AI Prompt** named **Summarize**: Provider `local`, Base URL `http://localhost:11434/v1`, your model name, and a prompt such as:
   ```
   Summarize this in two sentences:

   Title: {{Newest Item.output.latest.title}}
   Link: {{Newest Item.output.latest.link}}

   {{Newest Item.output.latest.summary}}
   ```
6. Add a **File** named **Append to Briefing**: Operation `append`, Path `daily-briefing.txt`, Content `{{Newest Item.output.latest.title}}` then `{{Summarize.output.content}}` and `{{Newest Item.output.latest.link}}`, one per line.
7. Add a **Desktop Notification** named **Notify Me**: Title `{{Newest Item.output.latest.title}}`, Body `{{Summarize.output.content}}`.
8. Connect: Every Morning → Fetch Feed → Newest Item → Anything New, then True → Summarize → Append to Briefing → Notify Me → an **Output** named **Briefing Done**.
9. Connect Anything New's False port to a **File** named **Log Nothing New** (Operation `append`, Path `daily-briefing.txt`, Content `{{format_date($run.timestamp, "YYYY-MM-DD")}}: nothing new` and a newline), then to an **Output** named **Nothing New**.
10. Connect Fetch Feed's Error port to a **Desktop Notification** named **Alert Fetch Failed**, and Summarize's Error port to a **File** named **Save Unsummarized** (append the title, a note and the link) and then a **Desktop Notification** named **Alert Summary Failed**.

## Edit before running

- **Feed URL**, in both Fetch Feed (URL) and Newest Item (Base URL).
- **Model**, in Summarize: a model you have pulled in Ollama. See [Local Models Quickstart](../docs/guide/local-models-quickstart.md).
- **File path**, if you want the briefing somewhere other than `daily-briefing.txt`.

No credentials are needed.

## Notes

- Press **Run** once with the **Output** drawer open. Expect the baseline run to show `count` `0`. To see an item go through, set On First Run to `emit` for that one test, or run it again after the feed publishes something new.
- Feed Parser's `summary` is plain text with the tags removed. Its `*_html` fields are raw feed HTML: do not paste them into an HTML email unescaped.
- HTTP Request decodes the response as UTF-8, so a feed in another encoding can arrive with replacement characters. See [Feed Parser](../docs/guide/nodes.md#feed-parser) for the limit.
- Checked against the workflow file rules; not yet imported into the real app.
