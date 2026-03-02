export type RotationMode = "random" | "sequential" | "shuffle";

export type Interval =
  | "five_minutes"
  | "fifteen_minutes"
  | "thirty_minutes"
  | "one_hour"
  | "four_hours"
  | "daily";

export interface Settings {
  stash_url: string;
  api_key: string;
  image_filter: string;
  rotation_mode: RotationMode;
  interval: Interval;
  per_monitor: boolean;
  wifi_only: boolean;
}

export const INTERVAL_LABELS: Record<Interval, string> = {
  five_minutes: "5 minutes",
  fifteen_minutes: "15 minutes",
  thirty_minutes: "30 minutes",
  one_hour: "1 hour",
  four_hours: "4 hours",
  daily: "Daily",
};

export const ROTATION_MODE_LABELS: Record<RotationMode, string> = {
  random: "Random",
  sequential: "Sequential",
  shuffle: "Shuffle (no repeat)",
};
