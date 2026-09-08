# Aerini Community & Development Model

Aerini uses a community-driven development model. Every user gets an equal voice in shaping priorities, whether or not they support the project financially. The goal is for Aerini to grow alongside the people who use it, not just get released to them.

---

## 1. Community Voting

All users can vote in polls on development priorities. Voting isn't restricted to Patreon supporters.

### Where Voting Happens

Every poll runs on GitHub Discussions and Patreon at the same time, not one or the other. Some users will never make a GitHub account, and some will never make a Patreon account, so requiring either one exclusively would quietly shut out whichever group doesn't use it.

Voting needs a free account on whichever platform is used. On GitHub, any signed-in user can vote, since the repo is public, that just means being signed in. On Patreon, voting needs at least a free membership on Aerini's page. Joining free costs nothing and unlocks the poll, no paid tier required.

Each platform's poll is its own count. The two tallies are read side by side, not merged into one number, and the developer weighs both when making the final call. Voting on both platforms means being counted in both tallies. That's a byproduct of running two open communities instead of one shared voter roll, not a loophole to worry about.

Once a poll launches, it stays open for one week, the same slot Week 2 fills in the example cycle (§10), though when a poll actually launches can vary. Patreon's poll tool supports a scheduled end date, so that's set directly on the post. GitHub polls don't have a scheduled close, so the discussion gets manually closed at the end of the week, which also turns off voting. Results, the leading option on each platform, are posted once the poll closes.

Users vote on things like:

- Which node to develop next
- Which integrations to prioritize
- Which quality-of-life improvements matter most
- Which features to consider for a future release
- Other community-driven development decisions

### Equal Voting

Each person gets one vote per platform they vote on. Financial support doesn't add voting power on either platform. Support buys content, not influence.

Neither platform checks for duplicate or alt accounts. That's a deliberate low-stakes tradeoff, not something worth building anti-fraud tooling for.

### Advisory, Not Binding

Polls guide development. They don't bind the developer. Final decisions may still depend on technical feasibility, security, maintenance burden, development time, compatibility, or overall direction, so the project isn't locked into a feature just because it won a poll.

---

## 2. Patreon

Patreon is the primary way to directly and financially support Aerini's day-to-day development and community. (A commercial license, covered in [FUNDING.md](FUNDING.md), is a separate thing — it's for removing AGPL-3.0's copyleft obligation, not for supporting the project as such.)

The paid benefit is new plugins, released on a bi-weekly to monthly cadence: new integrations, workflow nodes, specialized utilities, experimental plugins, community-requested plugins. Exact plugins and the cadence within that window can vary.

**Does not provide:**

- More votes
- Greater roadmap influence
- Priority ownership of feature decisions
- Control over Aerini's direction

**Also used for:**

- Monthly development updates
- Upcoming feature previews
- What's being worked on next
- New items added in the latest update
- Behind-the-scenes development posts
- Info on upcoming plugins

---

## 3. Instagram & Direct Communication

Instagram is an informal way to reach the developer directly. It's useful for beginners, non-technical users, or anyone who'd rather not open a GitHub issue. A user can just say "I have an idea for Aerini" and start a conversation without needing to learn GitHub's workflow.

GitHub is still the place for technical conversations and bug reports. Instagram is just a lower-friction alternative for casual suggestions and quick questions.

### Discoverability

The handle lives on the website's Contact page, along with a short note on what it's for. It's not in the sitewide footer. A bare footer link gives no context and just invites low-intent clicks and spam, while the Contact page means someone actually has to go looking for it, which keeps it visible to the people it's meant for. Patreon can mirror the handle, but the Contact page is the source of truth.

---

## 4. GitHub

The technical home of Aerini: source code, issues, bug reports, technical discussions, contributions, development docs, releases.

Each platform serves a different audience. GitHub isn't being replaced by Patreon or Instagram.

- **GitHub:** building Aerini
- **Patreon:** supporting Aerini and following development
- **Instagram:** talking directly with the person building it

---

## 5. Feature Suggestion → Community Vote → Development

```text
User suggests an idea
        ↓
Idea is reviewed
        ↓
Useful / realistic ideas are considered for a poll
        ↓
Community votes
        ↓
Winning ideas influence the roadmap
        ↓
Development
        ↓
Feature / plugin is released
        ↓
Community is informed
```

**Example:** a user suggests a PostgreSQL node. Other users show interest, it goes into a poll, the community votes. If it wins strong support and is technically reasonable, it gets prioritized. On release: "You asked for it, PostgreSQL support is now available in Aerini."

If a winning idea doesn't pan out during development, that's said, with the reason, when it happens. The roadmap gets updated accordingly, not left showing something that quietly stalled.

---

## 6. Community Roadmap

A public roadmap tracks development status:

- 🟢 **Completed:** released
- 🟡 **In Development:** currently being worked on
- 🔵 **Planned:** intended for future development
- 🗳️ **Community Voting:** ideas currently being considered

Kept simple. Not meant to become a full project-management board.

---

## 7. Why This Model Matters

```text
Developer decides → Developer builds → Users receive update
```
becomes
```text
Users suggest → Community discusses/votes → Developer evaluates → Aerini evolves
```

It can surface features the developer hadn't considered, helps prioritize what users actually want, and keeps development a bit more transparent.

---

## 8. Core Philosophy

Three principles: open to everyone regardless of payment, an equal vote per user, and financial support that earns extra content, not extra influence.

---

## 9. Platform Roles at a Glance

| Platform | Primary Purpose | Audience |
|---|---|---|
| **GitHub** | Code, issues, technical discussions, contributions, polls (mirrored) | Developers / contributors |
| **Patreon** | Support, roadmap updates, polls (mirrored), plugins (bi-weekly/monthly) | Users + supporters |
| **Instagram** | Direct communication, suggestions, casual updates | Everyone |
| **Website** | Product information, documentation, downloads, roadmap, Instagram/Patreon contact links | Everyone |

Polls run on both GitHub and Patreon at the same time (see §1). Neither platform owns polling exclusively.

---

## 10. Example Monthly Cycle

This is one illustrative cycle, not a fixed schedule. Real timing depends on the feature and how much bandwidth there is in a given month.

- **Week 1, Feedback:** collect suggestions via Instagram, Patreon, GitHub, and other channels.
- **Week 2, Poll:** select a few realistic ideas, community votes (poll stays open the full week, see §1).
- **Week 3, Development:** build the winning idea from the poll, or fold it into existing work.
- **Week 4, Release:** ship the feature, publish an update explaining what changed.

Any phase can run long. If feedback or the poll takes a few extra weeks because the maintainer is busy, the rest of the cycle just shifts, there's no monthly release commitment being made here. A feature too big to finish in one cycle doesn't get forced into a deadline either: it stays 🟡 In Development (§6) for as many cycles as it takes.

Plugin releases follow their own bi-weekly-to-monthly cadence and don't need to line up with this cycle, though they sometimes land in the same window.

---

## Final Concept

Anyone can use Aerini, suggest improvements, vote on priorities, and contact the developer directly. Anyone who supports it financially also gets plugins on a bi-weekly-to-monthly cadence. The goal is an open-source project shaped by its users, built sustainably by the person maintaining it.
