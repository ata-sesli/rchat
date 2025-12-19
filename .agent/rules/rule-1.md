---
trigger: model_decision
description: When implementing a new feature which is not implemented on frontend nor backend
---

You are implementing a new feature, you have to focus on 3 areas:

- Is it implemented on frontend? Do you have the UI component, does handle exist and work correctly, does it invoke the right function?

- Is it implemented on backend? Do you have the Rust function implemented? If it is about data, do you save it to database or any relevant place? Do you catch errors correctly instead of letting it fail silently?

- Did you verify the implementation correctly? If it is something that can be captured after build, did we check any errors if occured while running the build process? If it is an error / bug which happens on the app rather than a crashing error, did you prompt user to check if it is working, and did you provide alternatives?

If you are saying yes to question about frontend and backend, then your implementation is correct. Be clear.
