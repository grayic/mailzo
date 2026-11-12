// Whether the AI that comes with Mailzo Cloud can be used here: the daily brief, questions
// about your email, AI sorting of new mail and the spam second opinion. Without it those
// show, switched off, with a short note saying where they come from.
import { useEffect, useState } from "react";
import { api } from "./api";

let known: boolean | null = null;
let asking: Promise<boolean> | null = null;
const listeners = new Set<(paid: boolean) => void>();

/** Asks again, such as after signing in or out. */
export function reloadPaid(): Promise<boolean> {
  asking ??= api
    .paidStatus()
    .then((s) => s.paid)
    .catch(() => false)
    .then((paid) => {
      known = paid;
      asking = null;
      listeners.forEach((l) => l(paid));
      return paid;
    });
  return asking;
}

export function usePaid(): boolean {
  const [paid, setPaid] = useState(known ?? false);
  useEffect(() => {
    listeners.add(setPaid);
    if (known === null) void reloadPaid();
    else setPaid(known);
    return () => {
      listeners.delete(setPaid);
    };
  }, []);
  return paid;
}
