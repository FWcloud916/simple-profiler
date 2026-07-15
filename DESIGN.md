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
    process-rank-1: "#D9485F"
    process-rank-2: "#0F8A72"
    process-rank-3: "#C26B00"
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
    process-rank-1: "#FF7F96"
    process-rank-2: "#57D3BC"
    process-rank-3: "#FFB45E"
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
Three dedicated comparison colors identify ranked process lines without borrowing severity colors.

## Colors

Both light and dark semantic palettes are implemented above. Primary actions, selection, normal
state, warning, and error MUST remain distinguishable using text or symbols in addition to color.
Ranked process lines additionally use `#1`–`#3` labels and solid/dashed/dotted patterns.

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
MUST be visible in both themes. The time navigator uses a retained-coverage track with a visible
selected-window marker, Earlier/Later/Live controls, and direct chart dragging; focused charts MUST
also support Left/Right/Home/End navigation. Chart inspection uses a vertical guide, point markers,
and a compact timestamp/value tooltip. CPU, memory, disk-I/O, and network charts MAY overlay
at most three bounded matching process series per dimension with a visible ranked legend. When a
system and process measure use different units, such as disk-space percent and writer B/s, the
process series MUST use a labeled attribution lane with its own scale.

## Responsive Behavior

The dashboard is a local web surface supporting widths from 360px upward. Charts remain full width,
tables scroll horizontally below 760px, the event drawer becomes a full-width sheet below 640px,
and touch targets remain at least 36px high. The time slider occupies its own row below 920px;
horizontal chart gestures MUST preserve normal vertical page scrolling on touch devices.

## Do's and Don'ts

- **Do** keep units, timestamps, sampling gaps, and unavailable capabilities visible.
- **Do** show the selected historical window and provide a one-action return to live data.
- **Do** separate observed evidence from inferred causes.
- **Do** provide non-color indicators for severity and chart series.
- **Do** report process memory as both system percentage and bytes in chart tooltips.
- **Do** label disk and network process attribution as host-wide when the system series is scoped
  to one mount or interface.
- **Don't** plot values with different units against one vertical scale.
- **Don't** hide peaks by showing only averages.
- **Don't** render unbounded process histories or imply that ranked colors represent severity.
- **Don't** issue an API request for every raw pointer movement; debounce timeline queries.
- **Don't** present planned collectors as currently available.
- **Don't** send telemetry or load remote visual assets without an explicit future decision.

## Agent Prompt Guide

Dashboard changes MUST preserve the tokens above, compact monitoring direction, both system
themes, explicit unavailable/missing-data states, non-color severity cues, and 360px minimum
layout. New colors, spacing, radii, or component families SHOULD update this contract in the same
change as the implementation.
