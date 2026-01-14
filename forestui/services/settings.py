"""Settings service for managing application configuration."""

from __future__ import annotations

import json
from pathlib import Path

from forestui.models import Settings

# Default forest directory
DEFAULT_FOREST_PATH = Path.home() / "forest"

# Runtime forest path (set at startup via CLI argument)
_forest_path: Path | None = None


def set_forest_path(path: Path | str | None) -> None:
    """Set the runtime forest path."""
    global _forest_path
    if path is None:
        _forest_path = DEFAULT_FOREST_PATH
    else:
        _forest_path = Path(path).expanduser().resolve()


def get_forest_path() -> Path:
    """Get the current forest path."""
    if _forest_path is None:
        return DEFAULT_FOREST_PATH
    return _forest_path


class SettingsService:
    """Service for managing application settings."""

    _instance: SettingsService | None = None
    _settings: Settings | None = None
    _config_path: Path = Path.home() / ".config" / "forestui" / "settings.json"

    def __new__(cls) -> SettingsService:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self) -> None:
        if self._settings is None:
            self._settings = self._load_settings()

    def _load_settings(self) -> Settings:
        """Load settings from config file."""
        if self._config_path.exists():
            try:
                with self._config_path.open(encoding="utf-8") as f:
                    data = json.load(f)
                    return Settings.model_validate(data)
            except (json.JSONDecodeError, OSError):
                pass
        return Settings.default()

    def save_settings(self, settings: Settings) -> None:
        """Save settings to config file."""
        self._config_path.parent.mkdir(parents=True, exist_ok=True)
        with self._config_path.open("w", encoding="utf-8") as f:
            json.dump(settings.model_dump(), f, indent=2)
        self._settings = settings

    @property
    def settings(self) -> Settings:
        """Get current settings."""
        if self._settings is None:
            self._settings = self._load_settings()
        return self._settings

    def update(self, **kwargs: str) -> None:
        """Update settings with new values."""
        current = self.settings.model_dump()
        current.update(kwargs)
        new_settings = Settings.model_validate(current)
        self.save_settings(new_settings)


def get_settings_service() -> SettingsService:
    """Get the singleton SettingsService instance."""
    return SettingsService()
