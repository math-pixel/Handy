import { create } from "zustand";

/**
 * Global state for audio import transcription.
 * Survives tab switches because it lives outside any component tree.
 */
interface TranscriptionImportStore {
  isImporting: boolean;
  setImporting: (v: boolean) => void;
}

export const useTranscriptionImportStore = create<TranscriptionImportStore>(
  (set) => ({
    isImporting: false,
    setImporting: (v) => set({ isImporting: v }),
  }),
);
