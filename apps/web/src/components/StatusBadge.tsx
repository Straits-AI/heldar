// Thin compatibility wrapper around the shared <StatusPill> primitive.
// Kept so existing pages (CameraCard, CameraDetail) keep compiling unchanged.

import type { CameraStatusState } from "../lib/types";
import { cx, StatusPill } from "./ui";

interface Props {
  state: CameraStatusState | string | undefined;
  className?: string;
}

export function StatusBadge({ state, className }: Props) {
  if (className) {
    return (
      <span className={cx("inline-flex", className)}>
        <StatusPill state={state ?? "unknown"} />
      </span>
    );
  }
  return <StatusPill state={state ?? "unknown"} />;
}

export default StatusBadge;
