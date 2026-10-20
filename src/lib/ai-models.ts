// Model choice for OpenAI. Kept apart from ai-agent.ts so that Settings can
// read it without pulling the whole agents SDK into the startup bundle.

export const AI_MODELS = [
  "gpt-5.1",
  "gpt-5",
  "gpt-5-mini",
  "gpt-4.1",
  "gpt-4.1-mini",
] as const

const AI_MODEL_STORAGE_KEY = "winclean-ai-model"
const DEFAULT_AI_MODEL = "gpt-5-mini"

export function getStoredAiModel(): string {
  const stored = localStorage.getItem(AI_MODEL_STORAGE_KEY)
  if (stored && (AI_MODELS as readonly string[]).includes(stored)) {
    return stored
  }
  return DEFAULT_AI_MODEL
}

export function setStoredAiModel(model: string) {
  localStorage.setItem(AI_MODEL_STORAGE_KEY, model)
}
