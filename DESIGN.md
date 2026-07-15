---
colors:
  light:
    primary: "TODO — not yet designed"
    primary-active: "TODO — not yet designed"
    background: "TODO — not yet designed"
    surface: "TODO — not yet designed"
    text-primary: "TODO — not yet designed"
    text-secondary: "TODO — not yet designed"
    border: "TODO — not yet designed"
    accent: "TODO — not yet designed"
    success: "TODO — not yet designed"
    warning: "TODO — not yet designed"
    error: "TODO — not yet designed"
  dark:
    primary: "TODO — not yet designed"
    primary-active: "TODO — not yet designed"
    background: "TODO — not yet designed"
    surface: "TODO — not yet designed"
    text-primary: "TODO — not yet designed"
    text-secondary: "TODO — not yet designed"
    border: "TODO — not yet designed"
    accent: "TODO — not yet designed"
    success: "TODO — not yet designed"
    warning: "TODO — not yet designed"
    error: "TODO — not yet designed"
typography:
  heading-1: { fontFamily: "system-ui", fontSize: "TODO — not yet designed", fontWeight: "TODO — not yet designed", lineHeight: "TODO — not yet designed", letterSpacing: "TODO — not yet designed" }
  heading-2: { fontFamily: "system-ui", fontSize: "TODO — not yet designed", fontWeight: "TODO — not yet designed", lineHeight: "TODO — not yet designed", letterSpacing: "TODO — not yet designed" }
  body: { fontFamily: "system-ui", fontSize: "TODO — not yet designed", fontWeight: "TODO — not yet designed", lineHeight: "TODO — not yet designed", letterSpacing: "TODO — not yet designed" }
  caption: { fontFamily: "system-ui", fontSize: "TODO — not yet designed", fontWeight: "TODO — not yet designed", lineHeight: "TODO — not yet designed", letterSpacing: "TODO — not yet designed" }
  button: { fontFamily: "system-ui", fontSize: "TODO — not yet designed", fontWeight: "TODO — not yet designed", lineHeight: "TODO — not yet designed", letterSpacing: "TODO — not yet designed" }
rounded:
  none: "0"
  sm: "TODO — not yet designed"
  md: "TODO — not yet designed"
  lg: "TODO — not yet designed"
  full: "9999px"
spacing:
  xs: "TODO — not yet designed"
  sm: "TODO — not yet designed"
  md: "TODO — not yet designed"
  lg: "TODO — not yet designed"
  xl: "TODO — not yet designed"
  xxl: "TODO — not yet designed"
components: {}
---
# Simple Profiler Design System

## Overview

The planned dashboard is a compact, information-dense monitoring interface. Its visual character
is restrained and functional so time-series evidence and abnormal periods remain the focus. Both
light and dark modes are required, using system fonts. Exact tokens remain undecided because no
brand assets, reference product, or color palette was supplied.

## Colors

Both light and dark semantic palettes are required. Primary actions, selection, normal state,
warning, and error MUST remain distinguishable without relying on color alone. Exact colors and
contrast targets are TODO — not yet designed.

## Typography

The dashboard uses system UI fonts to avoid external font loading. Heading, body, caption, and
button sizes and weights are TODO — not yet designed. Numeric metrics SHOULD use tabular figures
when the selected system font and frontend implementation support them.

## Layout

The confirmed direction is compact rather than airy. The dashboard SHOULD prioritize a time-range
control, health summary, event timeline, metric charts, and process evidence. Exact grid,
breakpoints, and spacing tokens are TODO — not yet designed.

## Elevation & Depth

TODO — not yet designed. Elevation SHOULD communicate overlays or focused inspection instead of
decorating every metric card.

## Shapes

TODO — not yet designed. Radius tokens remain placeholders until the dashboard frontend is chosen.

## Components

Planned component classes include time-range controls, metric summary cards, time-series charts,
event markers, process tables, empty states, and data-unavailable states. Component tokens and
interaction states are TODO — not yet designed.

## Responsive Behavior

The dashboard is planned as a local web surface, but minimum viewport, breakpoint, and mobile
behavior are TBD — not yet designed.

## Do's and Don'ts

- **Do** keep units, timestamps, sampling gaps, and unavailable capabilities visible.
- **Do** separate observed evidence from inferred causes.
- **Do** provide non-color indicators for severity and chart series.
- **Don't** hide peaks by showing only averages.
- **Don't** present planned collectors as currently available.
- **Don't** send telemetry or load remote visual assets without an explicit future decision.

## Agent Prompt Guide

Before generating dashboard code, an agent MUST resolve the TODO tokens with the user and update
this file. Generated UI SHOULD follow the compact monitoring direction, support both themes, and
show explicit unavailable and missing-data states.

