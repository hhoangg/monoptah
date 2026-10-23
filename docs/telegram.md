# Telegram questions

When an agent asks you a question and you are away, Monoptah can forward it to a
Telegram group so you can answer from your phone. Only questions are forwarded.
Everything else stays in the app.

## Setup

Telegram is set up per project. Right-click a project and choose **Telegram…**.

1. **Create a bot.** Open [@BotFather](https://t.me/BotFather) in Telegram, send
   `/newbot`, and follow the prompts. Copy the bot token it gives you.
2. **Save the token.** Paste it into the **Bot token** field and select **Save**.
   The token is checked with Telegram and the bot name appears. The token is stored
   only on this computer and is never shown again.
3. **Enable Topics in the group.** Create a Telegram group (or use an existing one),
   open the group settings, and turn on **Topics**. Each session gets its own topic.
4. **Make the bot an admin with Manage topics.** The bot needs this right to create
   a topic per session. The **Connect a group** link asks for it when you add the bot.
5. **Connect the group.** Select **Connect a group**, then **Open Telegram** and pick
   the group. If the link does not work, send the shown command (for example
   `/connect@your_bot CODE`) in the group instead. The dialog switches to
   "Connected to <group>" once the bot is in.
6. **Turn on Forward questions.** Connecting a group does not start forwarding. Switch
   on **Forward questions**; it starts off. Switch it off again at any time to pause
   forwarding without disconnecting.
7. **Check the status.** The dialog shows whether Topics are on and whether the bot
   can manage topics, and explains what to fix if not.

To use a topic you already have, open the session's menu and choose **Link Telegram
topic…**. The dialog shows a command like `/link@your_bot CODE`. Send it inside the
topic you want to use (not in General); the bot confirms in that topic and the dialog
shows **Linked**. The code is valid for 15 minutes; select **New code** if it expires.
You can link a session before or after it first asks a question. The menu item appears
only for sessions of a project that has a bot and a connected group, and not for remote
or Mono sessions or open terminal (TUI) sessions. A terminal session that is closed and
only saved may still show it, but its questions are never forwarded.

## Keep the group private

Anyone in the connected group can answer the agent. A tap on an option, or a text
reply to a question, becomes the agent's answer, and the bot does not check who sent
it. Only add people you trust to the group.

## How it behaves

- A question is forwarded only after it has waited **5 minutes** without an answer.
  Questions answered, skipped or cancelled sooner are never sent.
- Each session gets **one topic**, created when its first question is forwarded and
  named `[ticket] title` (or just the title when no ticket is linked).
- **Any member** of the group can answer.
- The **first answer wins**. If you answer on the desktop first, the Telegram message
  is marked "Answered on desktop". If someone answers on Telegram first, the desktop
  question is answered with that reply.
- Choose options with the buttons. For a free-text answer, tap **Other (reply with
  text)** on the question message, then **reply to that message** with your text. Only
  questions that allow a custom answer have that button.
- A prompt with several questions is sent as one message per question, each with its
  own **Skip**.

## Limits

- Telegram rate limits apply. When Telegram asks the app to slow down, messages are
  delayed and sent later.
- **The app must stay open.** Monoptah reads your answers while it is running. Nothing
  is forwarded or received while it is closed.
- **Questions only.** Permission or tool approvals and plans are not forwarded and
  cannot be answered from Telegram.
- **Not forwarded:** remote (SSH) projects, TUI sessions, Mono chats, and
  orchestration workers (their lead answers them). The project menu hides
  **Telegram…** for remote projects.
- Topic names are cut to Telegram's 128 character limit.

## Do not run two apps on one bot

A bot can be read by only one app at a time. If you run two Monoptah apps with the
same bot token (for example the installed app and a dev build), Telegram rejects the
second reader with a conflict (HTTP 409) and the dialog shows **Another app is reading
this bot**. Close one of them, or give the second its own bot.

If a webhook is set on the bot, Telegram will not deliver updates to the app. The
dialog shows **This bot has a webhook** with a **Remove webhook** button.
