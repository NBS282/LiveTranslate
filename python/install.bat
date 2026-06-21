@echo off
setlocal

set "ROOT=%~dp0.."
set "VENV=%ROOT%\.venv-engine"

echo [1/4] Checking Python...
python --version >nul 2>&1
if errorlevel 1 (
    echo ERROR: Python not found. Install Python 3.10+ from https://python.org and try again.
    exit /b 1
)

echo [2/4] Creating virtual environment...
if not exist "%VENV%\Scripts\python.exe" (
    python -m venv "%VENV%"
    if errorlevel 1 ( echo ERROR: Failed to create venv. && exit /b 1 )
)

echo [3/4] Installing packages (this may take 10-20 minutes)...
"%VENV%\Scripts\pip" install --upgrade pip
"%VENV%\Scripts\pip" install torch --index-url https://download.pytorch.org/whl/cpu
if errorlevel 1 ( echo ERROR: Failed to install torch. && exit /b 1 )
"%VENV%\Scripts\pip" install "nemo_toolkit[asr]" transformers sentencepiece piper-tts "fastapi[standard]" "uvicorn[standard]" soundfile
if errorlevel 1 ( echo ERROR: Failed to install packages. && exit /b 1 )

echo [4/4] Done. Run the app to download models on first use.
