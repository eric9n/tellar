import os
import json
import base64
import uuid
import subprocess
import sys

def main():
    try:
        args = json.loads(os.environ.get("TELLAR_ARGS", "{}"))
        prompt = args.get("prompt", "")
        
        api_key = os.environ.get("GEMINI_API_KEY")
        
        if not api_key:
            print("Error: Missing GEMINI_API_KEY")
            sys.exit(1)
            
        if not prompt:
            print("Error: No prompt provided")
            sys.exit(1)

        # 1. Generate Image (Imagen 3)
        model = "imagen-3.0-generate-002"
        endpoint = f"https://generativelanguage.googleapis.com/v1beta/models/{model}:predict?key={api_key}"
        
        payload = {
            "instances": [{"prompt": prompt}]
        }
        
        process = subprocess.run(
            ["curl", "-s", "-X", "POST", "-H", "Content-Type: application/json", "-d", json.dumps(payload), endpoint],
            capture_output=True, text=True
        )
        
        if process.returncode != 0:
            print(f"Error calling Gemini: {process.stderr}")
            sys.exit(1)
            
        res_data = json.loads(process.stdout)
        if "predictions" not in res_data:
            print(f"Error: No predictions in Gemini response: {process.stdout}")
            sys.exit(1)
            
        b64_data = res_data["predictions"][0]["bytesBase64Encoded"]
        image_bytes = base64.b64decode(b64_data)
        
        # 2. Save image to brain/attachments
        filename = f"gen_{uuid.uuid4()}.png"
        brain_dir = os.path.join("brain", "attachments")
        os.makedirs(brain_dir, exist_ok=True)
        rel_path = os.path.join(brain_dir, filename)
        
        # We assume the current working directory is the guild root when skills are run
        with open(rel_path, "wb") as f:
            f.write(image_bytes)
            
        # 3. Return result with local file link for Steward perception
        print(f"🎨 Successfully generated image for: {prompt}")
        print(f"File saved to: brain/attachments/{filename} (local: [file://{rel_path}])")

    except Exception as e:
        print(f"Unexpected error: {e}")
        sys.exit(1)

if __name__ == "__main__":
    main()
