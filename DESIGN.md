---
colors:
  light:
    primary: "#246BFD"
    primary-active: "#1854D8"
    background: "#F4F7FB"
    surface: "#FFFFFF"
    text-primary: "#172033"
    text-secondary: "#667085"
    border: "#DFE5EE"
    accent: "#6D5DFB"
    success: "#16855B"
    warning: "#B54708"
    error: "#C62828"
  dark:
    primary: "#79A7FF"
    primary-active: "#A6C4FF"
    background: "#0B1020"
    surface: "#141B2D"
    text-primary: "#E8EDF7"
    text-secondary: "#9AA8BF"
    border: "#2A3550"
    accent: "#A99BFF"
    success: "#5AD6A0"
    warning: "#FFB86B"
    error: "#FF7B7B"
typography:
  heading-1: { fontFamily: "system-ui", fontSize: "32px", fontWeight: "720", lineHeight: "1.15", letterSpacing: "-0.025em" }
  heading-2: { fontFamily: "system-ui", fontSize: "20px", fontWeight: "680", lineHeight: "1.25", letterSpacing: "-0.015em" }
  body: { fontFamily: "system-ui", fontSize: "14px", fontWeight: "400", lineHeight: "1.5", letterSpacing: "0" }
  caption: { fontFamily: "system-ui", fontSize: "12px", fontWeight: "500", lineHeight: "1.4", letterSpacing: "0.01em" }
  button: { fontFamily: "system-ui", fontSize: "13px", fontWeight: "650", lineHeight: "1", letterSpacing: "0" }
rounded:
  none: "0"
  sm: "6px"
  md: "10px"
  lg: "14px"
  full: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
  xxl: "32px"
components:
  panel: { background: "{colors.light.surface}", radius: "{rounded.lg}", padding: "{spacing.lg}" }
  control: { radius: "{rounded.md}", height: "36px", paddingInline: "{spacing.md}" }
  badge: { radius: "{rounded.full}", paddingInline: "{spacing.sm}" }
---
# Simple Profiler Design System

## Overview

The dashboard is a compact, information-dense monitoring interface. Its visual character
is restrained and functional so time-series evidence and abnormal periods remain the focus. Both
light and dark modes are required, using system fonts. It uses blue for primary data, violet for
secondary comparison, green for healthy state, amber for warning, and red for critical state.

## Colors

Both light and dark semantic palettes are implemented above. Primary actions, selection, normal
state, warning, and error MUST remain distinguishable using text or symbols in addition to color.

## Typography

The dashboard uses system UI fonts to avoid external font loading. Numeric metrics use tabular
figures when the selected system font supports them.

## Layout

The direction is compact rather than airy. The dashboard prioritizes a sticky time-range control,
health summary, event timeline, metric charts, and process evidence. The content width is 1440px;
summary cards use four columns above 920px, two columns below, and one column below 560px.

## Elevation & Depth

Panels use borders and minimal shadow. Elevated depth is reserved for the event-detail drawer and
focused controls instead of decorating every metric card.

## Shapes

Controls use 10px radii, panels 14px, and status badges full-pill radii.

## Components

Component classes include time-range controls, metric summary cards, time-series charts, event
markers, a keyboard-accessible event drawer, sortable process tables, loading skeletons, empty
states, and explicit data-unavailable states. Hover, focus, selected, warning, and critical states
MUST be visible in both themes.

## Responsive Behavior

The dashboard is a local web surface supporting widths from 360px upward. Charts remain full width,
tables scroll horizontally below 760px, the event drawer becomes a full-width sheet below 640px,
and touch targets remain at least 36px high.

## Do's and Don'ts

- **Do** keep units, timestamps, sampling gaps, and unavailable capabilities visible.
- **Do** separate observed evidence from inferred causes.
- **Do** provide non-color indicators for severity and chart series.
- **Don't** hide peaks by showing only averages.
- **Don't** present planned collectors as currently available.
- **Don't** send telemetry or load remote visual assets without an explicit future decision.

## Agent Prompt Guide

Dashboard changes MUST preserve the tokens above, compact monitoring direction, both system
themes, explicit unavailable/missing-data states, non-color severity cues, and 360px minimum
layout. New colors, spacing, radii, or component families SHOULD update this contract in the same
change as the implementation.
