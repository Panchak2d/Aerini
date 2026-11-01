# Example — Private Daily Briefing (Feed Parser → AI)

Reads an RSS or Atom feed every morning, takes the newest item you have not seen yet, has a local model summarize it in two sentences, and appends the result to a text file and shows a notification. No Code node, no cloud AI, no API key.

## Nodes used

| Node | Purpose |
|------|---------|
| Schedule (Every Morning) | Cron `0 8 * * *` |
| HTTP Request (Fetch Feed) | `GET` the feed. The URL in the file is a placeholder: replace it |
| Feed Parser (Newest Item) | Text `{{Fetch Feed.output.body}}`, Base URL the feed's address, Only New on, On First Run `baseline`, Max Items `1` |
| If / Condition (Anything New) | Condition `{{Newest Item.output.count}} > 0`; only the True port is wired |
| AI Prompt (Summarize) | Provider `local`, Base URL `http://localhost:11434/v1`, Model `llama3` (edit to a model you have pulled) |
| File (Append to Briefing) | Operation `append`, Path `daily-briefing.txt` |
| Desktop Notification (Notify Me) | Shows the title and the summary |

## How it behaves

- **First run is silent.** With On First Run `baseline`, Feed Parser remembers every item currently in the feed and returns none, so `count` is `0`, the If sends nothing down True, and the run ends. From the next run, only items that appeared since then come through.
- **One item per run.** Max Items `1` returns the newest unseen item. If two new items appear in one day, the second is returned on the next run, because only returned items are marked as seen.
- **Quiet days stay quiet.** No new item means `count` is `0`, so the model is not called and nothing is written.
- **Delivered at most once.** Feed Parser marks an item as seen when it runs. If the model is not running when the workflow fires, that item is not offered again.

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
8. Connect: Every Morning → Fetch Feed → Newest Item → Anything New, then True → Summarize → Append to Briefing → Notify Me.

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
