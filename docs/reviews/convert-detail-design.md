# Convert JSON detail fix: mini-design

## Contract
- `compute_detail_layout(input) -> (x, y, width, height)` uses logical coordinates throughout. It keeps the overlay unchanged, prefers the right side and fits the detail into the available work area. Short content is centered vertically; work-area-height content starts at the work area's top.
- `should_hide_for_focus(overlay_focused, detail_focused, outside_foreground) -> bool` only hides when focus has left both windows and the platform confirms another foreground. Windows compares HWNDs; Linux compares the X11 foreground PID. Unknown foreground does not hide. Both windows' blur events schedule a deferred focus recheck; no reentrant window operations occur inside the callback.
- `json_detail_show` passes the overlay's physical position to layout. The platform layer resolves the monitor at that point and converts all inputs to its logical coordinate space; the final position uses physical pixels so a detail window previously on a different DPI monitor does not apply its old scale.
- `json_detail_show(content)` updates only the detail window; an error is returned to the caller, with no overlay resize.

## Tradeoff
- On displays where a centered 720px overlay leaves less than 280px on both sides, the detail may overlap it. On 1366px and wider screens the detail shrinks to the available side space so both panels remain separate. Moving or resizing the overlay would violate the main-panel requirement.

## Tests
- Layout: standard and offset monitors, DPI, narrow displays, short and tall content.
- Focus: overlay-to-detail and detail-to-overlay do not hide; external focus does.
- Frontend: selecting JSON opens detail; selecting another result hides it.

## Review
- Independent review found two blockers: deferred two-window focus checks and physical-to-logical conversion. Both are incorporated above.
- Implementation review found four further risks: IPC call ordering, transient empty focus, mixed-DPI placement and wrapped lines. Show/hide calls are serialized; focus requires external foreground confirmation; monitor resolution uses the overlay position; height includes estimated wrapping at the actual detail width.
- Second review confirmed the Windows fixes and identified Linux's missing foreground confirmation. Linux now checks the active window PID; absent PID leaves focus unchanged rather than treating a transition as an external click.
