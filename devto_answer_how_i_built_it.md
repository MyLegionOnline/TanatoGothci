# How I Built It — short answer for the dev.to contest form

> Paste-ready text. Touches nothing else; purely a copy-and-paste answer.

---

**The whole project is built around Legion — a multi-agent platform I deployed locally.**

Legion (github.com/MyLegionOnline/legion-hub) is the harness that does the heavy lifting: it runs the agents, gives each one its tools, and keeps the whole workflow together. No agent framework from scratch — the platform is the backbone, and my project lives on top of it.

For the brains, every agent in the project is configured with an open-weight model routed through **OpenRouter**:

- **deepseek 4 flash**
- **deepseek 4.1 flash** (OpenRouter)
- **glm-5.3 flash** (OpenRouter)

Routing the open models through OpenRouter keeps things simple — a single OpenAI-compatible endpoint, and Legion just hands each agent its model.

One deliberate choice: the entire implementation stays **inside Legion**. I used only its standard tools and built-in workflow — no external MCP servers, no bolted-on integrations. The platform is the harness, the open-weight models are the interchangeable minds, and the whole thing fits in one local Legion instance with zero extra infrastructure.
