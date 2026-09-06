# Example — AI Pipeline (Schedule → HTTP → AI Prompt → Notion)

Pulls the latest Hacker News top story every day, summarises it with AI, and creates a Notion page.

## Nodes used

| Node | Purpose |
|------|---------|
| Schedule | Trigger once daily |
| HTTP Request (Story IDs) | Fetch the current top-stories ID list |
| HTTP Request (Story Detail) | Fetch the top story's own title and URL |
| AI Prompt | Summarise the story in 3 bullet points |
| Notion | Create a new page in a database |

Two HTTP Request nodes are used here — give them distinct names (as in the table above) when you add them. Two nodes left with the default name "HTTP Request" would be ambiguous: any expression referencing `HTTP Request.output...` always resolves to whichever one comes first in the workflow, silently ignoring the second.

## Expressions

- HTTP Request (Story Detail) URL: `https://hacker-news.firebaseio.com/v0/item/{{HTTP Request (Story IDs).output.body[0]}}.json`
- AI Prompt input: `"Summarise this article in 3 bullet points:\n\nTitle: {{HTTP Request (Story Detail).output.body.title}}\nURL: {{HTTP Request (Story Detail).output.body.url}}"`
- Notion page title: `{{HTTP Request (Story Detail).output.body.title}}`
- Notion "Summary" property: `{{AI Prompt.output.content}}`

Every expression needs the `.output.` segment between the node name and the field path — `{{HTTP Request (Story Detail).body.title}}` (without `.output.`) is rejected and replaced with an empty string.

## How to build it

1. Add a **Schedule** node. Cron: `0 9 * * *` (09:00 daily).
2. Add an **HTTP Request** node and rename it **HTTP Request (Story IDs)**.
   - URL: `https://hacker-news.firebaseio.com/v0/topstories.json`
   - This returns a plain JSON array of story IDs. `body[0]` is the current top story's ID.
3. Add a second **HTTP Request** node and rename it **HTTP Request (Story Detail)**.
   - URL: `https://hacker-news.firebaseio.com/v0/item/{{HTTP Request (Story IDs).output.body[0]}}.json`
   - This returns an object with `title`, `url`, and other item fields.
4. Add an **AI Prompt** node. Select your provider and model. Set Prompt to the expression above.
5. Add a **Notion** node. Set Action to **create_page**. Set Database ID to your target database.
   - Title: the expression above — this fills the database's title property.
   - Properties: the Notion node has no separate "page body" field — it only ever writes to the target database's own properties (plus the title). To store the summary, add a rich-text column to your Notion database (e.g. named **Summary**) and set Properties to:
     ```json
     {
       "Summary": {
         "rich_text": [
           { "text": { "content": "{{AI Prompt.output.content}}" } }
         ]
       }
     }
     ```
     The property name in this JSON (`Summary`) must exactly match an existing column in your database — the API can fill an existing property, not create a new one on the fly.
6. Connect: Schedule → HTTP Request (Story IDs) → HTTP Request (Story Detail) → AI Prompt → Notion.

## Credentials needed

Add a credential for each secret under **Settings → Credentials → Add a credential** (pick any Type — API Key works for both; the type is just a label for your own reference and doesn't change how the field behaves), then select it from the relevant node's **Use Saved Credential** dropdown:

- An OpenAI or Anthropic API key, for the AI Prompt node's `api_key` field.
- A Notion integration token (`secret_...`), for the Notion node's `api_key` field.

## Notes

- This uses two chained HTTP Request nodes — one to fetch the ID list, one to fetch the item. Give them distinct names so expressions unambiguously target the right one.
- AI Prompt supports multiple providers. Switch the model in the node config without rewiring.
- AI Prompt's output field is `content` — not `text` or `response`.
