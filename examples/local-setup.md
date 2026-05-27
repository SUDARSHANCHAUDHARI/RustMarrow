# Local Setup Example

Marrow reads credentials from your local environment. Keep real values outside git.

```bash
mkdir -p ~/.marrow
cp .env.example ~/.marrow/.env
$EDITOR ~/.marrow/.env
```

Minimal required variables:

```dotenv
GITHUB_TOKEN=your_github_pat_here
GITHUB_USERNAME=SUDARSHANCHAUDHARI
ANTHROPIC_API_KEY=your_anthropic_key_here
```

Optional source-specific variables can be added later for Gmail, Calendar, Slack, and Obsidian sync.
