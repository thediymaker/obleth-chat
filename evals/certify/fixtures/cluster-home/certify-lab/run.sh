#!/bin/bash
# Trains the reef model on the lab's survey data and writes checkpoints to runs/.
#SBATCH --job-name=reef-train
#SBATCH --partition=general
#SBATCH --cpus-per-task=4
#SBATCH --mem=8G
#SBATCH --time=02:00:00
module load python/3.11
cd "$HOME/certify-lab"
python train.py --data survey.csv --epochs 20 --checkpoint-dir runs/
