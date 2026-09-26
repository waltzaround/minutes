import type { CapabilityProfile, Headline, Rating } from "@/lib/types";

export const ratingLabel: Record<Rating, string> = {
  excellent: "Excellent",
  good: "Good",
  standard: "Standard",
  limited: "Limited",
  unavailable: "Not available",
};

export const ratingTone: Record<Rating, string> = {
  excellent: "text-success",
  good: "text-success",
  standard: "text-foreground",
  limited: "text-warning",
  unavailable: "text-muted-foreground",
};

export const headlineTitle: Record<Headline, string> = {
  ready: "Your computer is ready",
  compatible: "Your computer is compatible",
  limited: "Limited performance",
  notSupported: "This computer can't run Minutes reliably",
};

export const modelNames: Record<string, string> = {
  "nemotron-3-nano-4b-q4km": "Nemotron 3 Nano 4B",
  "nemotron-3-nano-30b-a3b-q4km": "Nemotron 3 Nano 30B",
};

export const profileLabel: Record<CapabilityProfile, string> = {
  basic: "Basic",
  standard: "Standard",
  enhanced: "Enhanced",
  full: "Full",
};
