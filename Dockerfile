FROM python:3.11-slim

# Prevent Python from writing .pyc files and enable unbuffered logging
ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PYTHONPATH=/app

WORKDIR /app

# Install system dependencies if required
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy dependency requirements
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy source code and default data
COPY academic_agent ./academic_agent
COPY sample_docs ./sample_docs

# Create runtime directories
RUN mkdir -p /app/data/calendar /app/tugas_mahasiswa /app/backup_tugas

EXPOSE 8000

# Default health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:8000/health || exit 1

# Start the FastAPI Webhook bridge server by default
CMD ["uvicorn", "academic_agent.server:app", "--host", "0.0.0.0", "--port", "8000"]
