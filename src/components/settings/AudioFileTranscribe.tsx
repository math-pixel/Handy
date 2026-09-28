import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";
import { SettingContainer } from "../ui/SettingContainer";
import { useTranscriptionImportStore } from "@/stores/transcriptionImportStore";

type Status = "idle" | "transcribing" | "success" | "error";

export const AudioFileTranscribe: React.FC = () => {
  const { t } = useTranslation();
  const { isImporting, setImporting } = useTranscriptionImportStore();
  const [status, setStatus] = useState<Status>(() =>
    useTranscriptionImportStore.getState().isImporting ? "transcribing" : "idle",
  );
  const [errorMsg, setErrorMsg] = useState("");

  // Sync local status when global import state changes (e.g. after remount)
  useEffect(() => {
    if (isImporting && status === "idle") {
      setStatus("transcribing");
    }
  }, [isImporting, status]);

  const handleClick = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: t("settings.audioFile.title"),
          extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac", "aiff", "aif", "opus"],
        },
      ],
    });

    if (!selected || typeof selected !== "string") return;

    const filePath = selected;

    setImporting(true);
    setStatus("transcribing");
    setErrorMsg("");

    try {
      await invoke<string>("transcribe_audio_file", { filePath });
      setImporting(false);
      setStatus("success");
      setTimeout(() => setStatus("idle"), 3000);
    } catch (e) {
      setImporting(false);
      setErrorMsg(String(e));
      setStatus("error");
      setTimeout(() => setStatus("idle"), 5000);
    }
  };

  const label = (() => {
    switch (status) {
      case "transcribing":
        return t("settings.audioFile.transcribing");
      case "success":
        return t("settings.audioFile.success");
      case "error":
        return t("settings.audioFile.error", { message: errorMsg });
      default:
        return t("settings.audioFile.button");
    }
  })();

  return (
    <SettingContainer
      title={t("settings.audioFile.title")}
      description={t("settings.audioFile.description")}
      descriptionMode="inline"
      grouped={true}
    >
      <Button
        variant="secondary"
        size="sm"
        onClick={handleClick}
        disabled={status === "transcribing"}
      >
        {label}
      </Button>
    </SettingContainer>
  );
};
