---
description: Fixing bugs that occurs in the application
---

While fixing a bug, consider these following steps:

- If there is a log, read the log carefully and proceed with step 2. If there isn't any logs and user does not describe the error, add minimal logging to understand the error.

- After understanding the error, proceed with reading the relevant files and understand the flow of execution, try to corner the exact location where error occurs as much as you can.

- After deciding on the location, think about how to fix it permanently and correctly. Do not apply temporary patches (unless you are trying to understand the error). Do not solve the problem badly, for example do not add sleep in order to fix a race condition. The solution must be clear and guaranteed. If you cannot solve the error in the way you thought, always consider alternatives and mention it to the user. If user asks you to solve in a certain way, follow the user.
