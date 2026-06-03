# Example — AI Pipeline (Schedule → HTTP → AI Prompt → Notion)

Pulls the latest Hacker News top story every day, summarises it with AI, and creates a Notion page.

## Nodes used

| Node | Purpose |
|------|---------|
| Schedule | Trigger once daily |
| HTTP Request | Fetch top HN story from the API |
| AI Prompt | Summarise the story in 3 bullet points |
| Notion | Create a new page in a database |

## Expressions

- AI Prompt input: `"Summarise this article in 3 bullet points:\n\nTitle: {{HTTP Request.body.title}}\nURL: {{HTTP Request.body.url}}"`
- Notion page title: `{{HTTP Request.body.title}}`
- Notion page body: `{{AI Prompt.text}}`

## How to build it

1. Add a **Schedule** node. Cron: `0 9 * * *` (09:00 daily).
2. Add an **HTTP Request** node.
   - URL: `https://hacker-news.firebaseio.com/v0/topstories.json`
   - This returns an array of IDs. Wire the first ID into a second HTTP Request:
     `https://hacker-news.firebaseio.com/v0/item/{{HTTP Request.body[0]}}.json`
3. Add an **AI Prompt** node. Select your model. Set the prompt using the expression above.
4. Add a **Notion** node. Set operation to **Create Page**. Select your database.
   Set title and body using expressions above.
5. Connect: Schedule → HTTP Request (IDs) → HTTP Request (item) → AI Prompt → Notion.

## Credentials needed

- OpenAI or Anthropic API key (Settings → Credentials → Add → OpenAI / Anthropic)
- Notion integration token (Settings → Credentials → Add → Notion)

## Notes

- This uses two chained HTTP Request nodes — one to fetch the ID list, one to fetch the item.
- AI Prompt supports multiple providers. Switch the model in the node config without rewiring.
