# AFUI Composition

Composition combines multiple AFUI producer surfaces into one ordinary AFUI `ui_snapshot`. It is host/tooling behavior, not a new wire message type.

No first-stage schema change is required. The composed output is a normal snapshot that any conforming host can render.

## First-Stage Algorithm

Given sources such as `ops=ops.afui.json`, `crm=customer.afui.json`, and `billing=billing.afui.json`:

1. Validate each input as a complete `ui_snapshot`.
2. Assign each source a namespace such as `ops`, `crm`, or `billing`.
3. Prefix document ids with the namespace: `orders_table` becomes `ops:orders_table`.
4. Prefix action ids the same way: `reply` becomes `ops:reply`.
5. Rewrite all action references in views to the prefixed ids.
6. Mount each source state under `state.<namespace>` in the composed snapshot.
7. Rewrite `state:` bindings to include the namespace: `state:orders` becomes `state:ops.orders`.
8. Leave `record:` and `input:` bindings unchanged because they are local to a rendered view/action flow.
9. Produce a generated `surface`, append namespaced source screens, and validate the composed document.

## Routing Actions

A composed host emits `user_action.action_id` with the prefixed id, for example `ops:reply`. A router can dispatch by namespace:

```text
ops:reply -> ops handler
crm:open_customer -> crm handler
billing:refund -> billing handler
```

The handler response can be patched back into the owning namespace or used to regenerate the composed snapshot. Composition is host or tooling behavior; a composer namespaces ids, actions, and state and emits a normal snapshot.

## Non-Goals

- No `fragment` message type in v1.
- No dynamic agent-invented full workflow as the first-stage product.
- No document-supplied code to bridge producers.
- No requirement that producers know they are being composed.
