import React, { useEffect, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import { useTranslation } from "react-i18next";
import { commands, type ModelInfo } from "@/bindings";

interface ModelPickerDropdownProps {
  disabled?: boolean;
  onSelect: (modelId: string, modelName: string) => void;
}

export const ModelPickerDropdown: React.FC<ModelPickerDropdownProps> = ({
  disabled,
  onSelect,
}) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    commands.getAvailableModels().then((res) => {
      if (res.status === "ok") {
        setModels(res.data.filter((m) => m.is_downloaded));
      }
    });
  }, [open]);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        disabled={disabled}
        title={t("settings.history.retranscribeWithModel")}
        className="p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20 text-text/50 hover:text-logo-primary"
      >
        <ChevronDown className="w-3 h-3" />
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 min-w-[180px] rounded-md border border-mid-gray/30 bg-background shadow-lg py-1">
          <p className="px-3 py-1 text-xs text-text/40 font-medium uppercase tracking-wide">
            {t("settings.history.selectModel")}
          </p>
          {models.length === 0 ? (
            <p className="px-3 py-2 text-xs text-text/50">
              {t("settings.history.noDownloadedModels")}
            </p>
          ) : (
            models.map((m) => (
              <button
                key={m.id}
                onClick={() => {
                  setOpen(false);
                  onSelect(m.id, m.name);
                }}
                className="w-full text-left px-3 py-1.5 text-sm text-text/80 hover:bg-mid-gray/10 hover:text-text transition-colors"
              >
                {m.name}
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
};
