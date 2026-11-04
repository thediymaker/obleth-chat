-- 2,048 output tokens was the old default, saved with model policies, preferences,
-- presets and chats. No limit (0) is the default now: the model's own applies.
UPDATE model_profiles SET defaults = jsonb_set(defaults, '{maxTokens}', '0') WHERE defaults->>'maxTokens' = '2048';
UPDATE user_preferences SET preferences = jsonb_set(preferences, '{maxTokens}', '0') WHERE preferences->>'maxTokens' = '2048';
UPDATE user_presets SET parameters = jsonb_set(parameters, '{maxTokens}', '0') WHERE parameters->>'maxTokens' = '2048';
UPDATE conversations SET settings = jsonb_set(settings, '{maxTokens}', '0') WHERE settings->>'maxTokens' = '2048';
