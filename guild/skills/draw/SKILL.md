---
name: "creative-steward"
tools:
  draw:
    description: "Generate a high-quality image using Gemini Imagen 3 based on a text prompt."
    shell: "draw.py"
    parameters:
      type: "object"
      properties:
        prompt: { "type": "string", "description": "A detailed description of the image to generate" }
        thread_id: { "type": "string", "description": "The path to the blackboard where the result should appear" }
      required: ["prompt", "thread_id"]
---
# Creative Steward Skill
This skill leverages Gemini Imagen to generate visual assets for the workspace.
