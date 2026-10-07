# ClickUp inbox

Open **Settings → Inbox → ClickUp**, paste a personal API token, and select
**Connect**. Create the token in ClickUp under **Settings → Apps → API Token**
(it starts with `pk_`). The token is validated before it is saved. OAuth apps are
not supported by this integration.

If the token reaches more than one workspace, a workspace picker appears. Choose
one and select **Connect** again. A token with a single workspace connects
straight away. To switch workspace later, disconnect and connect again.

Select the ClickUp source in Inbox to browse tasks, read the Markdown description
and comments, post comments, ask about a task, or start work in a local project.
Starting work and asking about a task include its title, its identifier (the
custom id when your workspace uses them, otherwise `#<id>`), and its description.
Each card shows the space and list the task belongs to.

Space selections are shared between Settings and the Inbox filter menu. Unchecked
spaces are excluded from fetching and background notifications. Only spaces
ClickUp currently lists can be selected.

The inbox loads up to 40 open tasks, or 100 when including closed tasks, ordered
by last update. ClickUp's API cannot sort or filter by assignee for this view, so
tasks are sorted locally and **Assigned to me** matches your ClickUp username
against the newest 100 workspace tasks. Your own tasks older than that window do
not appear under that filter. The API is paged up to 500 tasks per refresh. Long
comment threads show the latest 50 comments.

Automations offer **ClickUp → Task appeared** after connecting. They run in the
automation's selected local project when a ClickUp task first appears in the
polled inbox, after its initial snapshot, once per task. The optional **Space**
filter lists spaces by name but stores the space id, so renaming a space in
ClickUp does not break an existing filter. This is polling, not a webhook for
every task created in the workspace.

**Disconnect** removes the saved token and clears cached ClickUp content. The
token is stored in `clickup-config.json` in the app's local data directory; on
Unix the file is created with owner-only permissions. When ClickUp rate limits
the app, requests pause until the limit resets and the inbox shows the error.
