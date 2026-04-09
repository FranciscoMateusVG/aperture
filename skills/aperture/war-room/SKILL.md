---
name: aperture-war-room
description: War Room participation protocol for Aperture agents. Use when invited to a War Room discussion, reading transcripts, or contributing to group discussions. Triggers on war room invitations, group discussions, and multi-agent deliberations.
---

# War Room Protocol

A War Room is a structured multi-agent discussion on a specific topic. This skill defines how to participate well — not just how to show up, but how to actually contribute.

---

## 1. Receiving an Invite

When a War Room context file appears at `/tmp/aperture-warroom-context.md`, read it immediately. It contains:

```
# WAR ROOM — [topic]
## Room: [id] | Round [n]

[instructions and transcript]

It is now YOUR turn ([your name]). Share your perspective.
```

**The file tells you when it's your turn.** Don't send a message to `warroom` unless the file ends with your name called. Wait for your turn.

---

## 2. Before You Respond — Read Everything

**Read the entire transcript before forming your response.** Not just the last message. The full thing.

Why: if you only react to the last message, you'll miss context, repeat things already said, or contradict a consensus that was already reached. This is the single most common failure mode in group discussions.

Transcript entry format:
```
[AGENT_NAME]: [their message]
[OPERATOR]: [human interjection, if any]
[SYSTEM]: [system events — war room start, round changes]
```

---

## 3. How to Contribute Well

**Build on what's already there.** Acknowledge good points explicitly. Challenge weak ones with reasoning, not just disagreement.

✅ Good:
> "GLaDOS's reorder is right — war room above spiderling because we all use war rooms. I'd also second Peppy's infra-handoff idea, and suggest we fold it into communicate rather than a standalone skill to keep the namespace lean."

❌ Bad:
> "I agree with everything said above. Sounds good to me."

**Introduce new angles** when you have them — don't just summarise what others said. Your job is to add signal, not echo it.

**Challenge respectfully.** If something is wrong or incomplete, say so and explain why. Vague agreement doesn't move a discussion forward.

**Stay on topic.** The War Room has a specific subject. Don't go off on tangents, don't bring up unrelated tasks, don't give status reports on other work.

---

## 4. Response Length

Target **150–400 words** per turn.

- Too short (< 100 words): you're not adding value, just acknowledging
- Too long (> 500 words): you're monologuing, not discussing

Structure your response if it covers multiple points. Use bold headers or numbered lists when listing action items or decisions. Keep prose tight.

---

## 5. Sending Your Response

**Always use `send_message(to: "warroom", message: "...")`**

Never reply to the war room in the terminal. The `warroom` recipient routes your message to the transcript and triggers the next agent's turn.

**One message per turn.** Don't send two messages in a row. If you forgot something, it waits until your next turn.

---

## 6. Operator Interjections

If `[OPERATOR]` appears in the transcript, the human has stepped in. Treat this as the highest priority input — address it directly in your next turn, even if it redirects the whole discussion.

---

## 7. Concluding a War Room

When the discussion has reached consensus and action items are clear, vote to conclude by including `[CONCLUDE]` in your message:

> "Great, we have consensus on all points. [CONCLUDE]"

**How it works:**
- Any agent can cast a conclude vote by including `[CONCLUDE]` anywhere in their warroom message
- The token is pattern-matched — it can appear mid-sentence or at the end
- Once **all participants** have included `[CONCLUDE]`, the War Room auto-concludes — no operator input needed
- The UI shows a `⚑ X/N want to conclude` indicator as votes accumulate
- The operator can still force-conclude at any time via the UI button

**Before voting `[CONCLUDE]`:**
- Summarise the agreed decisions
- Confirm who owns what action items
- Make sure no open questions remain

After the War Room concludes, start working on your assigned action items immediately.

---

## 8. Anti-Patterns

| Anti-pattern | Why it's bad |
|---|---|
| Replying in the terminal | Nobody else sees it |
| Sending multiple messages per turn | Breaks the round-robin flow |
| Only reading the last message | You miss context and repeat things |
| "I agree with everything above" | Zero signal added |
| Monologuing 600+ words | Kills discussion momentum |
| Ignoring operator interjections | The human is in the room — listen |
| Voting `[CONCLUDE]` before consensus | Leaves unresolved questions hanging |
